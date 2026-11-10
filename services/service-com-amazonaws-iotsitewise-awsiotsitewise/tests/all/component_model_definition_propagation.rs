// SPDX-License-Identifier: CC0-1.0

//! What a change to a reused `COMPONENT_MODEL` does to the assets that already
//! instantiate the models reusing it, end to end over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule under test: what a component-model-based composite model contributes
//! is read from the `COMPONENT_MODEL`'s stored record rather than copied when the
//! composite model was created, so `UpdateAssetModel` on that component model is a
//! change to the property definitions of every asset model reusing it — and
//! "Updates an asset model and all of the assets that were created from the model.
//! Each asset created from the model inherits the updated asset model's property
//! and hierarchy definitions" therefore reaches those assets too: a definition
//! added to the component model becomes a property of each of them, under the
//! composite model that reuses it, and one removed from it stops being a property
//! of them.
//!
//! Asserted over a TABLE whose rows are what the rule generalises over: how many
//! steps there are between the component model and the asset — a composite model
//! that reuses it directly, a SECOND asset model reusing the same component model
//! (so one update reaches assets of both), and a composite model nested inside
//! another one — crossed with the change made (a definition added, then one
//! removed).
//!
//! The statuses and refusals written out here come from
//! `smithy_query(query="operation", operation="UpdateAssetModel,DescribeAssetProperty")`
//! on this crate's `smithy-model/`: `UpdateAssetModel` answers 202 and
//! `DescribeAssetProperty` models `ResourceNotFoundException` (404).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, error_code, property_id, router};
use crate::asset_model_composite_model::{create_composite, PAST_THE_OWED_TRANSITION};

/// The property every row's component model starts with, so a row's subject is
/// what the UPDATE does rather than whether the component model contributed
/// anything at all.
const KEPT_PROPERTY: &str = "Flow";

/// The definition each row adds to its component model after the asset exists.
const ADDED_PROPERTY: &str = "Torque";

/// One row of the table: how the asset comes by the component model's definitions.
struct Row {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// Whether the composite model that reuses the component model is nested
    /// inside another composite model of the asset model.
    nested: bool,
    /// Whether a SECOND asset model reuses the same component model, so one update
    /// has to reach the assets of both.
    second_reuser: bool,
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a top-level composite model reusing the component model",
            nested: false,
            second_reuser: false,
        },
        Row {
            case: "two asset models reusing the same component model",
            nested: false,
            second_reuser: true,
        },
        Row {
            case: "a nested composite model reusing the component model",
            nested: true,
            second_reuser: false,
        },
    ]
}

/// Create a `COMPONENT_MODEL` carrying `properties` and return its id.
async fn component_model(router: &MultiServiceRouter, name: &str, properties: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {created}"))
        .to_string()
}

/// Create an asset model with one base property and return its id.
async fn host_model(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelProperties": [
                {"name": "Uptime", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {created}"))
        .to_string()
}

/// Create a composite model on `model_id` and return its id.
async fn composite(
    router: &MultiServiceRouter,
    case: &str,
    model_id: &str,
    body: Value,
) -> String {
    let (status, created) = create_composite(router, model_id, body).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModelCompositeModel got {status} {created}"
    );
    created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no composite model id in {created}"))
        .to_string()
}

/// The composite model that reuses `component_id` on `model_id`, nested inside a
/// `CUSTOM` composite model when the row asks for it. Answers its id.
async fn reusing_composite(
    router: &MultiServiceRouter,
    row: &Row,
    model_id: &str,
    component_id: &str,
    name: &str,
) -> String {
    let mut body = json!({
        "assetModelCompositeModelName": name,
        "assetModelCompositeModelType": "CUSTOM",
        "composedAssetModelId": component_id,
    });
    if row.nested {
        let parent = composite(
            router,
            row.case,
            model_id,
            json!({
                "assetModelCompositeModelName": format!("{name}Parent"),
                "assetModelCompositeModelType": "CUSTOM",
            }),
        )
        .await;
        body["parentAssetModelCompositeModelId"] = json!(parent);
    }
    composite(router, row.case, model_id, body).await
}

/// `GET /asset-models/{model_id}/composite-models/{composite_id}`, as the
/// model-side view of what a composite model contributes.
async fn model_composite(
    router: &MultiServiceRouter,
    case: &str,
    model_id: &str,
    composite_id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/asset-models/{model_id}/composite-models/{composite_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAssetModelCompositeModel got {status} {body}"
    );
    body
}

/// `GET /assets/{asset_id}/properties/{property_id}`.
async fn describe_property(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties/{property_id}"),
        None,
    )
    .await
}

/// The property ids `ListAssetProperties` reports for an asset, over every
/// property it carries (`filter=ALL`).
async fn listed_property_ids(router: &MultiServiceRouter, case: &str, asset_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAssetProperties got {status} {body}"
    );
    body["assetPropertySummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `PUT /asset-models/{component_id}` carrying exactly `properties`.
async fn update_component_model(
    router: &MultiServiceRouter,
    case: &str,
    component_id: &str,
    name: &str,
    properties: Value,
) {
    // A property this component model already holds is resubmitted by its stored
    // id, so the update KEEPS it instead of asking for the replacement
    // `UpdateAssetModel` refuses; see `crate::support::named_by_stored_id`.
    let properties = crate::support::named_by_stored_id(router, component_id, properties).await;
    let (status, body) = call(
        router,
        "PUT",
        &format!("/asset-models/{component_id}"),
        Some(json!({
            "assetModelName": name,
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: UpdateAssetModel got {status} {body}"
    );
}

/// The definition of one property of the component model.
fn definition(name: &str, unit: &str) -> Value {
    json!({ "name": name, "dataType": "DOUBLE", "unit": unit, "type": {"measurement": {}} })
}

/// RULE: a definition added to a reused `COMPONENT_MODEL` becomes a property of
/// every asset whose model reuses it — reported by `DescribeAssetProperty` under
/// the composite model that reuses it and listed by `ListAssetProperties` — and a
/// definition removed from that component model stops being a property of those
/// assets, whichever way and however many models reach it.
#[tokio::test]
async fn a_change_to_a_reused_component_model_reaches_the_assets_that_instantiate_it() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let component_name = "Pump";
        let component_id = component_model(
            &router,
            component_name,
            json!([definition(KEPT_PROPERTY, "m3/s")]),
        )
        .await;

        // One or two asset models reusing that component model, each with its own
        // asset: a row with a second reuser asserts one update reaches both.
        let mut hosts: Vec<(String, String)> = Vec::new();
        let host_count = if row.second_reuser { 2 } else { 1 };
        for index in 0..host_count {
            let model_id = host_model(&router, &format!("Plant{index}")).await;
            let composite_id = reusing_composite(
                &router,
                &row,
                &model_id,
                &component_id,
                &format!("Reused{index}"),
            )
            .await;
            hosts.push((model_id, composite_id));
        }

        // The models have to have settled before an asset can instantiate one.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let mut assets: Vec<(String, String, String)> = Vec::new();
        for (index, (model_id, composite_id)) in hosts.iter().enumerate() {
            let asset_id = create_asset(&router, &format!("Plant{index}-1"), model_id).await;
            assets.push((model_id.clone(), composite_id.clone(), asset_id));
        }

        // Before the update: the component model's one definition is a property of
        // every asset, which is what makes the rows below about the CHANGE.
        for (model_id, composite_id, asset_id) in &assets {
            let kept = property_id(
                &model_composite(&router, case, model_id, composite_id).await
                    ["assetModelCompositeModelProperties"],
                KEPT_PROPERTY,
            );
            let (status, body) = describe_property(&router, asset_id, &kept).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the component model's property must be the asset's before \
                 the update, got {status} {body}"
            );
        }

        // A definition ADDED to the component model after the assets exist.
        update_component_model(
            &router,
            case,
            &component_id,
            component_name,
            json!([
                definition(KEPT_PROPERTY, "m3/s"),
                definition(ADDED_PROPERTY, "Nm"),
            ]),
        )
        .await;
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        let mut added_ids: Vec<(String, String)> = Vec::new();
        for (model_id, composite_id, asset_id) in &assets {
            let added = property_id(
                &model_composite(&router, case, model_id, composite_id).await
                    ["assetModelCompositeModelProperties"],
                ADDED_PROPERTY,
            );
            let (status, body) = describe_property(&router, asset_id, &added).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a definition added to the reused component model must be a \
                 property of asset {asset_id}, got {status} {body}"
            );
            assert_eq!(
                body["compositeModel"]["id"].as_str(),
                Some(composite_id.as_str()),
                "case {case:?}: the added property must be reported under the composite \
                 model that reuses the component model, got {body}"
            );
            assert_eq!(
                body["compositeModel"]["assetProperty"]["unit"].as_str(),
                Some("Nm"),
                "case {case:?}: the added property must carry its definition's unit, got \
                 {body}"
            );
            let listed = listed_property_ids(&router, case, asset_id).await;
            assert!(
                listed.contains(&added),
                "case {case:?}: ListAssetProperties must report the added property {added} \
                 of asset {asset_id}, got {listed:?}"
            );
            added_ids.push((asset_id.clone(), added));
        }

        // And a definition REMOVED from the component model stops being a property
        // of those assets: "If you remove a property from an asset model, IoT
        // SiteWise deletes all previous data for that property".
        update_component_model(
            &router,
            case,
            &component_id,
            component_name,
            json!([definition(KEPT_PROPERTY, "m3/s")]),
        )
        .await;
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        for (asset_id, added) in &added_ids {
            let (status, body) = describe_property(&router, asset_id, added).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a definition removed from the reused component model must \
                 stop being a property of asset {asset_id}, got {status} {body}"
            );
            let listed = listed_property_ids(&router, case, asset_id).await;
            assert!(
                !listed.contains(added),
                "case {case:?}: ListAssetProperties must stop reporting the removed \
                 property {added} of asset {asset_id}, got {listed:?}"
            );
        }
    }
}
