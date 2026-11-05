// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `DeleteAssetModel` does to the models that
//! implemented the model it removes, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: removing the application of an interface asset
//! model to an asset model is a write to THAT asset model, however the removal
//! is reached, so the model carries a new version and a new ETag, reports
//! `UPDATING` until the transition the clock owes falls due, and keeps every
//! property it held — deleting the interface asset model removes every one of
//! its applications, so every model that implemented it is written exactly that
//! way, while a model that implemented nothing of it, and the interface side of
//! an application whose APPLIED-TO model is the one deleted, stay on the version
//! they were on.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, router};
use super::asset_model_interface_relationship::{
    create_model, describe_model, id_of, ids, property, relationship_uri,
    PAST_THE_OWED_TRANSITION,
};
use super::hierarchies_etags_and_refusals::describe_e_tag;

/// How the applications under test resolved their mappings: the three ways
/// `PutAssetModelInterfaceRelationship`'s configuration pairs an interface
/// property with a property of the model it is applied to.
#[derive(Debug, Clone, Copy)]
enum Mapping {
    /// `matchByPropertyName`: onto properties the model already held.
    ByName,
    /// `createMissingProperty`: onto properties the application created.
    Created,
    /// `overrides`: onto the properties the request named outright.
    Overridden,
}

/// Which `CustomID` form the delete addresses the interface asset model in.
#[derive(Debug, Clone, Copy)]
enum Label {
    /// The raw id the create minted.
    Raw,
    /// `externalId:` followed by the interface's external id.
    External,
}

/// One row of the table: the interface to build, how many models implement it
/// and with which configuration, and the label form the delete names it by.
#[derive(Debug)]
struct Case {
    /// What the row varies; every assertion carries it.
    case: &'static str,
    /// The properties the INTERFACE asset model declares.
    interface_properties: &'static [&'static str],
    /// The properties each implementing asset model declares.
    model_properties: &'static [&'static str],
    /// How many asset models implement the interface.
    implementers: usize,
    mapping: Mapping,
    label: Label,
}

/// The rows: one per thing the rule generalises over — how the applications
/// resolved their mappings (so, whether the applied-to models hold properties
/// the application created), how many models implement the interface, how many
/// properties the interface declares (none included), and the `CustomID` form
/// the delete addresses the interface in.
const CASES: &[Case] = &[
    Case {
        case: "one model, one property matched by name, deleted by raw id",
        interface_properties: &["Temperature"],
        model_properties: &["Temperature", "InletTemp"],
        implementers: 1,
        mapping: Mapping::ByName,
        label: Label::Raw,
    },
    Case {
        case: "two models, two properties created by the application, deleted by externalId label",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["Serial"],
        implementers: 2,
        mapping: Mapping::Created,
        label: Label::External,
    },
    Case {
        case: "three models, one property mapped by an override, deleted by raw id",
        interface_properties: &["Temperature"],
        model_properties: &["InletTemp", "Serial"],
        implementers: 3,
        mapping: Mapping::Overridden,
        label: Label::Raw,
    },
    Case {
        case: "one model, an interface declaring no property at all, deleted by raw id",
        interface_properties: &[],
        model_properties: &["Serial"],
        implementers: 1,
        mapping: Mapping::ByName,
        label: Label::Raw,
    },
];

/// The external id the doomed interface carries, so the delete can address it
/// by its `externalId:` label as well as by the raw id.
const INTERFACE_EXTERNAL_ID: &str = "interface.doomed";

/// Create an asset model of `model_type` named `name`, carrying one measurement
/// property per name in `properties` and the external id `external_id` when one
/// is given, and return its id.
async fn model_under(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    external_id: Option<&str>,
    model_type: &str,
    properties: &[&str],
) -> String {
    let mut body = json!({
        "assetModelName": name,
        "assetModelType": model_type,
        "assetModelProperties": properties
            .iter()
            .map(|name| property(name))
            .collect::<Vec<Value>>(),
    });
    if let Some(external_id) = external_id {
        body["assetModelExternalId"] = json!(external_id);
    }
    create_model(router, case, body).await
}

/// Apply `interface` to `asset_model` under `configuration`, asserting the
/// modelled 202 the application answers with.
async fn apply(
    router: &MultiServiceRouter,
    case: &str,
    asset_model: &str,
    interface: &str,
    configuration: Value,
) {
    let (status, body) = call(
        router,
        "PUT",
        &relationship_uri(asset_model, interface),
        Some(json!({ "propertyMappingConfiguration": configuration })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: the application got {status} {body:?}"
    );
}

/// The configuration one row applies its interface with, built against the two
/// models' stored property ids.
async fn configuration_for(
    router: &MultiServiceRouter,
    row: &Case,
    interface: &str,
    asset_model: &str,
) -> Value {
    match row.mapping {
        Mapping::ByName => json!({ "matchByPropertyName": true }),
        Mapping::Created => json!({ "createMissingProperty": true }),
        Mapping::Overridden => {
            let interface_properties =
                describe_model(router, row.case, interface).await["assetModelProperties"].clone();
            let model_properties =
                describe_model(router, row.case, asset_model).await["assetModelProperties"].clone();
            let overrides: Vec<Value> = row
                .interface_properties
                .iter()
                .zip(row.model_properties.iter())
                .map(|(interface_property, model_property)| {
                    json!({
                        "interfaceAssetModelPropertyId":
                            id_of(&interface_properties, row.case, interface_property),
                        "assetModelPropertyId":
                            id_of(&model_properties, row.case, model_property),
                    })
                })
                .collect();
            json!({ "overrides": overrides })
        }
    }
}

/// `DELETE /asset-models/{label}`.
async fn delete_model(router: &MultiServiceRouter, label: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/asset-models/{label}"), None).await
}

/// `GET` the relationship between two `CustomID` labels.
async fn read_relationship(
    router: &MultiServiceRouter,
    asset_model: &str,
    interface: &str,
) -> (u16, Value) {
    call(router, "GET", &relationship_uri(asset_model, interface), None).await
}

/// What one asset model reads as before and after a delete elsewhere: the
/// version, the ETag, the reported state and the ids of its properties.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    version: String,
    e_tag: String,
    state: String,
    properties: BTreeSet<String>,
}

/// Read `model_id` through `DescribeAssetModel` and its `ETag` header.
async fn view(router: &MultiServiceRouter, case: &str, model_id: &str) -> View {
    let body = describe_model(router, case, model_id).await;
    View {
        version: body["assetModelVersion"].as_str().unwrap_or("").to_string(),
        e_tag: describe_e_tag(router, model_id).await,
        state: body["assetModelStatus"]["state"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        properties: ids(&body["assetModelProperties"]),
    }
}

#[tokio::test]
async fn deleting_an_interface_model_writes_every_model_that_implemented_it() {
    for row in CASES {
        let case = row.case;
        let router = router();
        let interface = model_under(
            &router,
            case,
            "Doomed",
            Some(INTERFACE_EXTERNAL_ID),
            "INTERFACE",
            row.interface_properties,
        )
        .await;
        // A second interface, applied to the first implementer: the delete names
        // neither half of that pair, so it has to survive — and the model it is
        // applied to must be written for the ONE application that went away.
        let other_interface = model_under(
            &router,
            case,
            "OtherInterface",
            Some("interface.other"),
            "INTERFACE",
            &[],
        )
        .await;

        let mut implementers: Vec<String> = Vec::new();
        for index in 0..row.implementers {
            implementers.push(
                model_under(
                    &router,
                    case,
                    &format!("AppliedTo{index}"),
                    None,
                    "ASSET_MODEL",
                    row.model_properties,
                )
                .await,
            );
        }
        // A model that implements nothing of the doomed interface: the delete is
        // not a write to it, so every value it reports has to stay.
        let bystander = model_under(
            &router,
            case,
            "Bystander",
            None,
            "ASSET_MODEL",
            row.model_properties,
        )
        .await;

        for asset_model in &implementers {
            let configuration = configuration_for(&router, row, &interface, asset_model).await;
            apply(&router, case, asset_model, &interface, configuration).await;
        }
        apply(
            &router,
            case,
            &implementers[0],
            &other_interface,
            json!({ "matchByPropertyName": true }),
        )
        .await;

        // Past the applications' own owed transitions, so what the models report
        // afterwards is owed by the DELETE.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let mut before: Vec<View> = Vec::new();
        for asset_model in &implementers {
            let reading = view(&router, case, asset_model).await;
            assert_eq!(
                reading.state, "ACTIVE",
                "case {case:?}: expected the applications to have settled on {asset_model}, \
                 got {reading:?}"
            );
            before.push(reading);
        }
        let bystander_before = view(&router, case, &bystander).await;
        for asset_model in &implementers {
            let (status, related) = read_relationship(&router, asset_model, &interface).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {asset_model} must implement the interface before the delete, \
                 got {status} {related:?}"
            );
        }

        let label = match row.label {
            Label::Raw => interface.clone(),
            Label::External => format!("externalId:{INTERFACE_EXTERNAL_ID}"),
        };
        let (status, deleted) = delete_model(&router, &label).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the delete got {status} {deleted:?}"
        );
        assert_eq!(
            deleted["assetModelStatus"]["state"].as_str(),
            Some("DELETING"),
            "case {case:?}: expected DELETING, got {deleted:?}"
        );

        // Every model that implemented the interface no longer does: the read of
        // the pair refuses.
        for asset_model in &implementers {
            let (status, read) = read_relationship(&router, asset_model, &interface).await;
            assert_eq!(
                status, 404,
                "case {case:?}: the application to {asset_model} must be gone, \
                 got {status} {read:?}"
            );
            assert_eq!(
                error_code(&read),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code in {read:?}"
            );
        }

        // And each of them was WRITTEN for it: a new version, a new ETag, the
        // owed transition pending, and every property it held still there.
        for (asset_model, before) in implementers.iter().zip(before.iter()) {
            let after = view(&router, case, asset_model).await;
            assert_ne!(
                after.version, before.version,
                "case {case:?}: expected {asset_model} to carry a new version, \
                 both {:?}",
                after.version
            );
            assert_ne!(
                after.e_tag, before.e_tag,
                "case {case:?}: expected {asset_model} to carry a new ETag, both {:?}",
                after.e_tag
            );
            assert_eq!(
                after.state, "UPDATING",
                "case {case:?}: expected {asset_model} to report UPDATING while the owed \
                 transition is pending, got {after:?}"
            );
            assert_eq!(
                after.properties, before.properties,
                "case {case:?}: expected {asset_model} to keep its properties {:?}, got {:?}",
                before.properties, after.properties
            );
        }

        // The application the delete did not name stands, and the model that
        // implemented nothing of the doomed interface was not written.
        let (status, kept) = read_relationship(&router, &implementers[0], &other_interface).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the other interface must survive, got {status} {kept:?}"
        );
        let bystander_after = view(&router, case, &bystander).await;
        assert_eq!(
            bystander_after, bystander_before,
            "case {case:?}: expected {bystander} to be untouched, got {bystander_after:?}"
        );

        // The transition the delete owed falls due on the clock, for each model.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        for asset_model in &implementers {
            let settled = view(&router, case, asset_model).await;
            assert_eq!(
                settled.state, "ACTIVE",
                "case {case:?}: expected the owed transition on {asset_model} to settle, \
                 got {settled:?}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_model_that_implements_an_interface_leaves_the_interface_where_it_was() {
    // The other side of the same removal: the model being deleted is the
    // APPLIED-TO one, whose definition is what the application changed, so the
    // interface asset model it implemented is not written at all.
    let case = "the applied-to model is the one deleted";
    let router = router();
    let interface = model_under(
        &router,
        case,
        "Interface",
        Some(INTERFACE_EXTERNAL_ID),
        "INTERFACE",
        &["Temperature"],
    )
    .await;
    let doomed = model_under(
        &router,
        case,
        "AppliedTo",
        None,
        "ASSET_MODEL",
        &["Temperature"],
    )
    .await;
    let kept = model_under(&router, case, "Kept", None, "ASSET_MODEL", &["Temperature"]).await;
    for asset_model in [&doomed, &kept] {
        apply(
            &router,
            case,
            asset_model,
            &interface,
            json!({ "matchByPropertyName": true }),
        )
        .await;
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let before = view(&router, case, &interface).await;
    let kept_before = view(&router, case, &kept).await;

    let (status, deleted) = delete_model(&router, &doomed).await;
    assert_eq!(
        status, 202,
        "case {case:?}: the delete got {status} {deleted:?}"
    );

    let after = view(&router, case, &interface).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the interface {interface} to be untouched, got {after:?}"
    );
    // The other model's application is untouched too, and that model with it.
    let (status, related) = read_relationship(&router, &kept, &interface).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the surviving application must still read, got {status} {related:?}"
    );
    assert_eq!(
        view(&router, case, &kept).await,
        kept_before,
        "case {case:?}: expected {kept} to be untouched by the delete of {doomed}"
    );
    // Only the deleted model's own application went away.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/interface/{interface}/asset-models"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListInterfaceRelationships got {status} {listed:?}"
    );
    let applied_to: BTreeSet<String> = listed["interfaceRelationshipSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary["id"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        applied_to,
        BTreeSet::from([kept.clone()]),
        "case {case:?}: expected only {kept} to still implement {interface}, got {applied_to:?}"
    );
}
