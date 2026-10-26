// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteAssetModelInterfaceRelationship`, driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * the delete removes the APPLICATION of one interface asset model to one
//!   asset model from the store, whatever the configuration that application
//!   resolved its mappings with and whichever `CustomID` form each half of the
//!   pair is addressed in, so the read of that pair refuses afterwards and the
//!   interface no longer lists the model — and every OTHER application, of
//!   another interface to the same model and of this interface to another model,
//!   stays exactly where it was;
//! * the delete is a write to the ASSET MODEL, so that model carries a new
//!   version and owes the transition back to `ACTIVE` whose state the delete
//!   reports, and the model keeps every property it held, the ones the
//!   application created included: the request names the relationship and
//!   nothing else;
//! * a pair nothing relates — either half naming no stored asset model, two
//!   stored models no application related, or a pair a delete already removed —
//!   is the modelled not-found, and leaves the store untouched;
//! * a deleted relationship can be applied again.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, is_modelled_id, router};
use super::asset_model_interface_relationship::{
    create_model, describe_model, id_of, ids, property, relationship_uri,
    PAST_THE_OWED_TRANSITION,
};

/// A well-formed `CustomID` no asset model in this suite is stored under.
const STRANGER: &str = "99999999-8888-4777-8666-555555555555";

/// How the application under test resolved the mappings the delete removes:
/// the three ways `PutAssetModelInterfaceRelationship`'s configuration pairs an
/// interface property with a property of the model it is applied to.
#[derive(Debug, Clone, Copy)]
enum Mapping {
    /// `matchByPropertyName`: onto the properties the model already held.
    ByName,
    /// `createMissingProperty`: onto properties the application created.
    Created,
    /// `overrides`: onto the properties the request named outright.
    Overridden,
}

/// Which `CustomID` form the delete addresses each half of the pair in.
#[derive(Debug, Clone, Copy)]
enum Labels {
    /// Both halves as the raw ids the creates minted.
    Raw,
    /// Both halves as `externalId:` followed by the external id.
    External,
    /// The model raw, the interface as an `externalId:` label.
    Mixed,
}

/// One row of the removal table: the two models to build, the configuration the
/// interface was applied with, and the label forms the delete addresses them in.
#[derive(Debug)]
struct Case {
    /// What the row varies; every assertion carries it.
    case: &'static str,
    /// The properties the INTERFACE asset model declares.
    interface_properties: &'static [&'static str],
    /// The properties the applied-to asset model declares.
    model_properties: &'static [&'static str],
    mapping: Mapping,
    labels: Labels,
}

/// The table: one row per way an application resolves its mappings, crossed
/// with the `CustomID` forms the delete can address the pair in, plus the row
/// where the interface declares no property at all and the relationship
/// therefore carries no mapping.
const CASES: &[Case] = &[
    Case {
        case: "one property matched by name, deleted by raw ids",
        interface_properties: &["Temperature"],
        model_properties: &["Temperature", "InletTemp"],
        mapping: Mapping::ByName,
        labels: Labels::Raw,
    },
    Case {
        case: "two properties created by the application, deleted by externalId labels",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["Serial"],
        mapping: Mapping::Created,
        labels: Labels::External,
    },
    Case {
        case: "two properties mapped by overrides, deleted by a mixed pair of labels",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["InletTemp", "InletPressure"],
        mapping: Mapping::Overridden,
        labels: Labels::Mixed,
    },
    Case {
        case: "an interface declaring no property, deleted by raw ids",
        interface_properties: &[],
        model_properties: &["Serial"],
        mapping: Mapping::ByName,
        labels: Labels::Raw,
    },
];

/// Create an asset model of `model_type` under `external_id`, carrying one
/// measurement property per name in `properties`, and return its id.
async fn model_under(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    external_id: &str,
    model_type: &str,
    properties: &[&str],
) -> String {
    create_model(
        router,
        case,
        json!({
            "assetModelName": name,
            "assetModelExternalId": external_id,
            "assetModelType": model_type,
            "assetModelProperties": properties
                .iter()
                .map(|name| property(name))
                .collect::<Vec<Value>>(),
        }),
    )
    .await
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
        "case {case}: the application got {status} {body:?}"
    );
}

/// `DELETE` the relationship between the two `CustomID` labels.
async fn delete_relationship(
    router: &MultiServiceRouter,
    asset_model: &str,
    interface: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &relationship_uri(asset_model, interface),
        None,
    )
    .await
}

/// `GET` the relationship between the two `CustomID` labels.
async fn read_relationship(
    router: &MultiServiceRouter,
    asset_model: &str,
    interface: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &relationship_uri(asset_model, interface),
        None,
    )
    .await
}

/// The `assetModelId`s `ListInterfaceRelationships` reports for `interface`.
async fn applied_to(
    router: &MultiServiceRouter,
    case: &str,
    interface: &str,
) -> BTreeSet<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/interface/{interface}/asset-models"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListInterfaceRelationships got {status} {body:?}"
    );
    body["interfaceRelationshipSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary["id"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The configuration a row applies its interface with, built against the two
/// models' stored property ids.
async fn configuration_for(
    router: &MultiServiceRouter,
    case: &Case,
    interface: &str,
    asset_model: &str,
) -> Value {
    match case.mapping {
        Mapping::ByName => json!({ "matchByPropertyName": true }),
        Mapping::Created => json!({ "createMissingProperty": true }),
        Mapping::Overridden => {
            let interface_properties = describe_model(router, case.case, interface).await
                ["assetModelProperties"]
                .clone();
            let model_properties = describe_model(router, case.case, asset_model).await
                ["assetModelProperties"]
                .clone();
            let overrides: Vec<Value> = case
                .interface_properties
                .iter()
                .zip(case.model_properties.iter())
                .map(|(interface_property, model_property)| {
                    json!({
                        "interfaceAssetModelPropertyId":
                            id_of(&interface_properties, case.case, interface_property),
                        "assetModelPropertyId":
                            id_of(&model_properties, case.case, model_property),
                    })
                })
                .collect();
            json!({ "overrides": overrides })
        }
    }
}

#[tokio::test]
async fn deleting_a_relationship_removes_only_the_pair_it_names() {
    for row in CASES {
        let case = row.case;
        let router = router();
        let interface = model_under(
            &router,
            case,
            "Interface",
            "interface.main",
            "INTERFACE",
            row.interface_properties,
        )
        .await;
        let asset_model = model_under(
            &router,
            case,
            "AppliedTo",
            "applied.main",
            "ASSET_MODEL",
            row.model_properties,
        )
        .await;
        // A second interface applied to the SAME asset model, and the row's
        // interface applied to a SECOND asset model: neither pair is what the
        // delete names, so both have to survive it.
        let other_interface = model_under(
            &router,
            case,
            "OtherInterface",
            "interface.other",
            "INTERFACE",
            &[],
        )
        .await;
        let other_model = model_under(
            &router,
            case,
            "OtherAppliedTo",
            "applied.other",
            "ASSET_MODEL",
            row.model_properties,
        )
        .await;

        let configuration = configuration_for(&router, row, &interface, &asset_model).await;
        apply(&router, case, &asset_model, &interface, configuration).await;
        apply(
            &router,
            case,
            &asset_model,
            &other_interface,
            json!({ "matchByPropertyName": true }),
        )
        .await;
        apply(
            &router,
            case,
            &other_model,
            &interface,
            json!({ "matchByPropertyName": true, "createMissingProperty": true }),
        )
        .await;

        // Past the applications' own owed transitions, so the state the delete
        // reports is the one the DELETE owes.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let before = describe_model(&router, case, &asset_model).await;
        let before_version = before["assetModelVersion"].as_str().unwrap_or("").to_string();
        let before_properties = ids(&before["assetModelProperties"]);
        assert_eq!(
            before["assetModelStatus"]["state"],
            json!("ACTIVE"),
            "case {case}: expected the applications to have settled, got {:?}",
            before["assetModelStatus"]
        );
        let (status, applied) = read_relationship(&router, &asset_model, &interface).await;
        assert_eq!(
            status, 200,
            "case {case}: the pair must be related before the delete, got {status} {applied:?}"
        );

        let (model_label, interface_label) = match row.labels {
            Labels::Raw => (asset_model.clone(), interface.clone()),
            Labels::External => (
                "externalId:applied.main".to_string(),
                "externalId:interface.main".to_string(),
            ),
            Labels::Mixed => (asset_model.clone(), "externalId:interface.main".to_string()),
        };
        let (status, deleted) =
            delete_relationship(&router, &model_label, &interface_label).await;
        assert_eq!(
            status, 202,
            "case {case}: the delete got {status} {deleted:?}"
        );
        // Both ids are reported as the STORED ids, whichever label form the
        // request addressed them in.
        assert_eq!(
            deleted["assetModelId"],
            json!(asset_model),
            "case {case}: expected assetModelId {asset_model}, got {:?}",
            deleted["assetModelId"]
        );
        assert_eq!(
            deleted["interfaceAssetModelId"],
            json!(interface),
            "case {case}: expected interfaceAssetModelId {interface}, got {:?}",
            deleted["interfaceAssetModelId"]
        );
        assert!(
            is_modelled_id(deleted["assetModelId"].as_str().unwrap_or("")),
            "case {case}: {:?} is not a modelled ID",
            deleted["assetModelId"]
        );
        assert_eq!(
            deleted["assetModelArn"], before["assetModelArn"],
            "case {case}: expected the model's own ARN {:?}, got {:?}",
            before["assetModelArn"], deleted["assetModelArn"]
        );
        assert_eq!(
            deleted["assetModelStatus"]["state"],
            json!("UPDATING"),
            "case {case}: expected UPDATING while the owed transition is pending, got {:?}",
            deleted["assetModelStatus"]
        );

        // The pair is no longer related: the read refuses, and the interface no
        // longer lists the model.
        let (status, read) = read_relationship(&router, &asset_model, &interface).await;
        assert_eq!(
            status, 404,
            "case {case}: the deleted pair must refuse, got {status} {read:?}"
        );
        assert!(
            error_code(&read).contains("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            error_code(&read)
        );
        assert_eq!(
            applied_to(&router, case, &interface).await,
            BTreeSet::from([other_model.clone()]),
            "case {case}: expected only {other_model} to still implement {interface}"
        );

        // Every other application stands.
        let (status, kept) = read_relationship(&router, &asset_model, &other_interface).await;
        assert_eq!(
            status, 200,
            "case {case}: the other interface on this model must survive, got {status} {kept:?}"
        );
        let (status, kept) = read_relationship(&router, &other_model, &interface).await;
        assert_eq!(
            status, 200,
            "case {case}: this interface on the other model must survive, got {status} {kept:?}"
        );

        // The model keeps every property it held — the ones the application
        // created included — and carries a new version for the write.
        let after = describe_model(&router, case, &asset_model).await;
        assert_eq!(
            ids(&after["assetModelProperties"]),
            before_properties,
            "case {case}: expected the model's properties to stay, got {:?}",
            after["assetModelProperties"]
        );
        assert_ne!(
            after["assetModelVersion"].as_str().unwrap_or(""),
            before_version,
            "case {case}: expected a new version, got {:?}",
            after["assetModelVersion"]
        );

        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let settled = describe_model(&router, case, &asset_model).await;
        assert_eq!(
            settled["assetModelStatus"]["state"],
            json!("ACTIVE"),
            "case {case}: expected the owed transition to settle, got {:?}",
            settled["assetModelStatus"]
        );

        // A deleted relationship can be applied again.
        apply(
            &router,
            case,
            &asset_model,
            &interface,
            json!({ "matchByPropertyName": true, "createMissingProperty": true }),
        )
        .await;
        let (status, reapplied) = read_relationship(&router, &asset_model, &interface).await;
        assert_eq!(
            status, 200,
            "case {case}: the pair must be relatable again, got {status} {reapplied:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_of_a_pair_nothing_relates_is_the_modelled_not_found() {
    let case = "refusals";
    let router = router();
    let interface = model_under(
        &router,
        case,
        "Interface",
        "interface.main",
        "INTERFACE",
        &["Temperature"],
    )
    .await;
    let asset_model = model_under(
        &router,
        case,
        "AppliedTo",
        "applied.main",
        "ASSET_MODEL",
        &["Temperature"],
    )
    .await;
    let plain = model_under(
        &router,
        case,
        "Plain",
        "plain.one",
        "ASSET_MODEL",
        &["Temperature"],
    )
    .await;
    let related = model_under(
        &router,
        case,
        "Related",
        "applied.other",
        "ASSET_MODEL",
        &["Temperature"],
    )
    .await;
    // The one relationship the store holds: no refusal below may touch it.
    apply(
        &router,
        case,
        &related,
        &interface,
        json!({ "matchByPropertyName": true }),
    )
    .await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let before = describe_model(&router, case, &asset_model).await;
    let before_version = before["assetModelVersion"].as_str().unwrap_or("").to_string();

    for (row, model_label, interface_label) in [
        (
            "an asset model that does not exist",
            STRANGER.to_string(),
            interface.clone(),
        ),
        (
            "an interface asset model that does not exist",
            asset_model.clone(),
            STRANGER.to_string(),
        ),
        (
            "an externalId label no asset model carries",
            "externalId:applied.absent".to_string(),
            interface.clone(),
        ),
        (
            "two stored models no application related",
            asset_model.clone(),
            interface.clone(),
        ),
        (
            "a model and a model that was never applied as an interface",
            asset_model.clone(),
            plain.clone(),
        ),
    ] {
        let (status, body) = delete_relationship(&router, &model_label, &interface_label).await;
        assert_eq!(
            status, 404,
            "case {case}/{row}: expected 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case}/{row}: expected ResourceNotFoundException, got {:?}",
            error_code(&body)
        );
        // A refusal writes nothing: the relationship the store does hold is
        // still readable and the model it named is still on its own version.
        let (status, kept) = read_relationship(&router, &related, &interface).await;
        assert_eq!(
            status, 200,
            "case {case}/{row}: the stored relationship must survive, got {status} {kept:?}"
        );
        let unchanged = describe_model(&router, case, &asset_model).await;
        assert_eq!(
            unchanged["assetModelVersion"].as_str().unwrap_or(""),
            before_version,
            "case {case}/{row}: a refused delete must not write, got {:?}",
            unchanged["assetModelVersion"]
        );
    }

    // The refusal names both halves of the pair as the request supplied them.
    let (_, unrelated) = delete_relationship(&router, &asset_model, &interface).await;
    let message = unrelated["message"].as_str().unwrap_or("");
    assert!(
        message.contains(&asset_model) && message.contains(&interface),
        "case {case}: expected the message to name both models, got {message:?}"
    );

    // Deleting the one relationship the store holds succeeds exactly once: the
    // second delete has nothing to remove.
    let (status, body) = delete_relationship(&router, &related, &interface).await;
    assert_eq!(
        status, 202,
        "case {case}: the first delete got {status} {body:?}"
    );
    let (status, body) = delete_relationship(&router, &related, &interface).await;
    assert_eq!(
        status, 404,
        "case {case}: the second delete got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {:?}",
        error_code(&body)
    );
    assert_eq!(
        applied_to(&router, case, &interface).await,
        BTreeSet::new(),
        "case {case}: expected no model to implement {interface} any more"
    );
}
