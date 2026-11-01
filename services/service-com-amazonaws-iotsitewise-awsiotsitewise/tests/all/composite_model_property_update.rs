// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateAssetProperty` over EVERY property an asset
//! carries, driven over HTTP through the same `MultiServiceRouter` the emulator
//! boots with.
//!
//! The rule this suite pins: a property id names ONE property of an asset —
//! whether the asset's model declares it directly or one of that model's
//! composite models contributes it — so the write addressed to
//! `(assetId, propertyId)` reaches exactly the property every read of that pair
//! reports, and every view of that property afterwards reports the alias,
//! notification state and unit the write set.
//!
//! Each suite is a TABLE whose rows differ along WHERE the property is declared
//! (on the model itself, on a composite model created with the model, on one
//! added afterwards, on one nested inside another, and on the component model a
//! component-model-based composite model reuses — including a definition two
//! composite models contribute), along its data type, along whether its
//! definition carries a unit, and along which `CustomID` form addresses it. The
//! asset's own property is the control row: every row must be served the same.
//!
//! The wire harness (`router`, `call`, `create_asset`, …) lives in the
//! `asset_lifecycle` suite, the composite-model create in
//! `asset_model_composite_model` and the notification-topic oracle in
//! `asset_property_update`; this file reuses all three rather than keeping a copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, error_code, property_id, router};
use super::asset_model_composite_model::create_composite;
use super::asset_property_update::expected_topic;

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One row of the table: a property of the asset under test, where it is
/// declared, how the row addresses it and what its definition carries.
struct Row {
    /// What the row varies: the case name every assertion carries.
    case: &'static str,
    /// The stored `propertyId`, as the model-side read reports it.
    property_id: String,
    /// The `CustomID` label this row addresses the property by: the raw id, or
    /// `externalId:<externalId>` for a definition carrying one.
    label: String,
    /// The unit the asset model property definition declares, which is what an
    /// omitted `propertyUnit` falls back to.
    definition_unit: Option<&'static str>,
}

/// Create a composite model on `model_id` from `body` and answer its id.
async fn composite_id(router: &MultiServiceRouter, model_id: &str, body: Value) -> String {
    let (status, created) = create_composite(router, model_id, body).await;
    assert_eq!(status, 202, "create composite model: got {status} {created}");
    created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create composite model: no id in {created}"))
        .to_string()
}

/// The id of the property named `name` that composite model `composite` of
/// `model_id` contributes, as `DescribeAssetModelCompositeModel` reports it.
async fn composite_property(
    router: &MultiServiceRouter,
    model_id: &str,
    composite: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/asset-models/{model_id}/composite-models/{composite}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAssetModelCompositeModel {composite}: got {status} {body}"
    );
    property_id(&body["assetModelCompositeModelProperties"], name)
}

/// `UpdateAssetProperty` on `(asset_label, property_label)` with `body`.
async fn update_property(
    router: &MultiServiceRouter,
    asset_label: &str,
    property_label: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/assets/{asset_label}/properties/{property_label}"),
        Some(body),
    )
    .await
}

/// The asset property `DescribeAssetProperty` reports for `(asset_id, label)`,
/// whichever of the two members carries it: `assetProperty` for one of the
/// asset's own, `compositeModel.assetProperty` for one a composite model
/// contributes.
async fn described_property(router: &MultiServiceRouter, asset_id: &str, label: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties/{label}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAssetProperty {label}: got {status} {body}"
    );
    let reported = body["assetProperty"].clone();
    if reported.is_object() {
        return reported;
    }
    let composite = body["compositeModel"]["assetProperty"].clone();
    assert!(
        composite.is_object(),
        "DescribeAssetProperty {label}: no asset property in {body}"
    );
    composite
}

/// Every `ListAssetProperties` summary of `asset_id` whose id is `property_id`,
/// with `filter=ALL` so the listing spans the composite models' properties too.
///
/// A definition two composite models contribute is carried once per composite
/// model, each instance under its own path and all of them sharing the
/// definition's id — so this answers more than one summary for such a property,
/// and every one of them is a view of the one property that id names.
async fn listed_instances(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListAssetProperties {asset_id}: got {status} {body}"
    );
    let summaries = body["assetPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetPropertySummaries in {body}"))
        .clone();
    let instances: Vec<Value> = summaries
        .into_iter()
        .filter(|summary| summary["id"] == json!(property_id))
        .collect();
    assert!(
        !instances.is_empty(),
        "ListAssetProperties must list the property {property_id} of asset {asset_id}"
    );
    instances
}

/// An asset carrying a property in every place an asset carries one, answered as
/// `(model_id, asset_id, rows)` with one row per property under test.
///
/// `label` keeps the names of two fixtures in one test binary apart.
async fn boiler(router: &MultiServiceRouter, label: &str) -> (String, String, Vec<Row>) {
    // A COMPONENT_MODEL with a property of its own, reused by TWO composite
    // models below: a component-model-based composite model stores no properties
    // and the component model it names provides them, so one definition reaches
    // the asset once per composite model that contributes it.
    let (status, component) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{label}Thermostat"),
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Measured", "dataType": "DOUBLE", "unit": "Celsius",
                 "externalId": format!("{label}-measured"), "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "create component model: got {status} {component}");
    let component_id = component["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create component model: no assetModelId in {component}"))
        .to_string();

    // The asset model: one property of its own and one composite model declared
    // with it.
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{label}Boiler"),
            "assetModelProperties": [
                {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius",
                 "type": {"measurement": {}}},
            ],
            "assetModelCompositeModels": [
                {"name": "Alarm", "type": "AWS/ALARM", "properties": [
                    {"name": "AlarmState", "dataType": "STRING", "type": {"measurement": {}}},
                ]},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel: no assetModelId in {created}"))
        .to_string();

    // Four more composite models, added after the model was created: a custom
    // one, one nested inside it, and two that reuse the same component model.
    let shift = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Shift",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Throughput", "dataType": "DOUBLE", "unit": "units/h",
                 "externalId": format!("{label}-throughput"), "type": {"measurement": {}}},
                {"name": "Cycles", "dataType": "INTEGER", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    let nested = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "NestedShift",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": shift,
            "assetModelCompositeModelProperties": [
                {"name": "Deep", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    let reused = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": component_id,
        }),
    )
    .await;
    let _reused_again = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "ReusedAgain",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": component_id,
        }),
    )
    .await;

    let asset_id = create_asset(router, &format!("{label}Boiler-1"), &model_id).await;

    // The asset's own property, read back from the service rather than assumed.
    let (status, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    let temperature = property_id(&asset["assetProperties"], "Temperature");

    // The composite model declared with the asset model, as the model reports it.
    let (status, model) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    let alarm = model["assetModelCompositeModels"]
        .as_array()
        .unwrap_or_else(|| panic!("no composite models in {model}"))
        .iter()
        .find(|composite| composite["name"] == json!("Alarm"))
        .unwrap_or_else(|| panic!("no composite model named Alarm in {model}"))
        .clone();
    let alarm_state = property_id(&alarm["properties"], "AlarmState");

    let throughput = composite_property(router, &model_id, &shift, "Throughput").await;
    let cycles = composite_property(router, &model_id, &shift, "Cycles").await;
    let deep = composite_property(router, &model_id, &nested, "Deep").await;
    let measured = composite_property(router, &model_id, &reused, "Measured").await;

    let rows = vec![
        Row {
            case: "the asset's own property",
            label: temperature.clone(),
            property_id: temperature,
            definition_unit: Some("Celsius"),
        },
        Row {
            case: "a STRING property of a composite model declared with the asset model",
            label: alarm_state.clone(),
            property_id: alarm_state,
            definition_unit: None,
        },
        Row {
            case: "a property of a composite model added to the model afterwards",
            label: throughput.clone(),
            property_id: throughput.clone(),
            definition_unit: Some("units/h"),
        },
        Row {
            case: "that same property addressed by its external id",
            label: format!("externalId:{label}-throughput"),
            property_id: throughput,
            definition_unit: Some("units/h"),
        },
        Row {
            case: "an INTEGER property of a composite model, its definition carrying no unit",
            label: cycles.clone(),
            property_id: cycles,
            definition_unit: None,
        },
        Row {
            case: "a property of a composite model nested inside another",
            label: deep.clone(),
            property_id: deep,
            definition_unit: None,
        },
        Row {
            case: "a property two composite models contribute from the component model they reuse",
            label: measured.clone(),
            property_id: measured.clone(),
            definition_unit: Some("Celsius"),
        },
        Row {
            case: "that reused property addressed by its external id",
            label: format!("externalId:{label}-measured"),
            property_id: measured,
            definition_unit: Some("Celsius"),
        },
    ];
    (model_id, asset_id, rows)
}

#[tokio::test]
async fn update_asset_property_writes_a_composite_models_property_like_the_assets_own() {
    let router = router();
    let (model_id, asset_id, rows) = boiler(&router, "Update").await;

    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        // A distinct alias per row, so a handler writing one row's binding onto
        // another row's property would be caught rather than agreeing with itself.
        let alias = format!("/plant/1/update/{at}");

        let (status, answer) = update_property(
            &router,
            &asset_id,
            &row.label,
            json!({
                "propertyAlias": alias,
                "propertyNotificationState": "ENABLED",
                "propertyUnit": "Kelvin",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAssetProperty got {status} {answer}"
        );

        // The read the operation's own documentation pairs the write with
        // reports exactly what was written.
        let described = described_property(&router, &asset_id, &row.property_id).await;
        assert_eq!(
            described.get("alias").and_then(Value::as_str),
            Some(alias.as_str()),
            "case {case:?}: expected alias {alias:?}, got {described}"
        );
        assert_eq!(
            described["notification"]["state"].as_str(),
            Some("ENABLED"),
            "case {case:?}: expected notification state ENABLED, got {described}"
        );
        assert_eq!(
            described["notification"]["topic"].as_str(),
            Some(expected_topic(&model_id, &asset_id, &row.property_id).as_str()),
            "case {case:?}: unexpected notification topic in {described}"
        );
        assert_eq!(
            described.get("unit").and_then(Value::as_str),
            Some("Kelvin"),
            "case {case:?}: expected unit Kelvin, got {described}"
        );

        // EVERY view of the property that id names reports the same binding —
        // including each instance of a definition two composite models
        // contribute, since the id names one property and one data stream.
        for instance in listed_instances(&router, &asset_id, &row.property_id).await {
            assert_eq!(
                instance.get("alias").and_then(Value::as_str),
                Some(alias.as_str()),
                "case {case:?}: a listed instance must report alias {alias:?}, got {instance}"
            );
            assert_eq!(
                instance["notification"]["state"].as_str(),
                Some("ENABLED"),
                "case {case:?}: a listed instance must report state ENABLED, got {instance}"
            );
            assert_eq!(
                instance.get("unit").and_then(Value::as_str),
                Some("Kelvin"),
                "case {case:?}: a listed instance must report unit Kelvin, got {instance}"
            );
        }

        // The alias the write bound ADDRESSES that property: it is what
        // associates the data stream the alias names with it.
        let (status, latest) = call(
            &router,
            "GET",
            &format!("/properties/latest?propertyAlias={alias}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the alias {alias} must address the property, got {status} {latest}"
        );

        // The overwrite: every member omitted takes its documented default — the
        // alias is removed, the state is DISABLED and the unit falls back to the
        // one the asset model property definition carries.
        let (status, answer) = update_property(&router, &asset_id, &row.label, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the clearing update got {status} {answer}"
        );
        let described = described_property(&router, &asset_id, &row.property_id).await;
        assert!(
            described.get("alias").is_none_or(Value::is_null),
            "case {case:?}: an omitted propertyAlias must remove the alias, got {described}"
        );
        assert_eq!(
            described["notification"]["state"].as_str(),
            Some("DISABLED"),
            "case {case:?}: an omitted notification state must be DISABLED, got {described}"
        );
        assert_eq!(
            described.get("unit").and_then(Value::as_str),
            row.definition_unit,
            "case {case:?}: expected the definition's unit {:?}, got {described}",
            row.definition_unit
        );
        for instance in listed_instances(&router, &asset_id, &row.property_id).await {
            assert!(
                instance.get("alias").is_none_or(Value::is_null),
                "case {case:?}: a listed instance must report no alias, got {instance}"
            );
            assert_eq!(
                instance.get("unit").and_then(Value::as_str),
                row.definition_unit,
                "case {case:?}: a listed instance must report the definition's unit {:?}, got {instance}",
                row.definition_unit
            );
        }
        let (status, body) = call(
            &router,
            "GET",
            &format!("/properties/latest?propertyAlias={alias}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a removed alias must address nothing, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn an_alias_a_composite_models_property_carries_cannot_be_taken_by_another_property() {
    let router = router();
    let (_, asset_id, rows) = boiler(&router, "Conflict").await;
    let alias = "/plant/2/conflict/stream";

    // The first row that is a composite model's property takes the alias; every
    // other property of the asset is then refused it, whichever place it is
    // declared in.
    let holder = &rows[1];
    let (status, answer) = update_property(
        &router,
        &asset_id,
        &holder.label,
        json!({"propertyAlias": alias}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first binding ({:?}) got {status} {answer}",
        holder.case
    );

    for contender in rows.iter().filter(|row| row.property_id != holder.property_id) {
        let case = contender.case;
        let (status, body) = update_property(
            &router,
            &asset_id,
            &contender.label,
            json!({"propertyAlias": alias}),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: an alias another property carries must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictingOperationException",
            "case {case:?}: unexpected error code in {body}"
        );
        let described = described_property(&router, &asset_id, &contender.property_id).await;
        assert!(
            described.get("alias").is_none_or(Value::is_null),
            "case {case:?}: a refused update must leave the property unaliased, got {described}"
        );
    }

    // The binding the refusals were measured against still stands.
    let described = described_property(&router, &asset_id, &holder.property_id).await;
    assert_eq!(
        described.get("alias").and_then(Value::as_str),
        Some(alias),
        "the first binding must survive the refusals, got {described}"
    );
}

#[tokio::test]
async fn a_property_id_no_composite_model_of_the_asset_contributes_is_still_a_refusal() {
    let router = router();
    let (_, asset_id, _) = boiler(&router, "Refusal").await;
    // A property declared by a composite model of ANOTHER asset model: a stored
    // property id, but not one of THIS asset's, so it resolves to nothing here.
    let (_, other_asset, other_rows) = boiler(&router, "Other").await;
    let foreign = other_rows
        .iter()
        .find(|row| row.case.contains("nested"))
        .unwrap_or_else(|| panic!("the fixture must carry a nested composite model's property"));

    let rows = [
        ("an id no create ever minted", ABSENT_ID.to_string()),
        (
            "a property of a composite model of another asset's model",
            foreign.property_id.clone(),
        ),
        (
            "an external id no definition of this asset carries",
            "externalId:no-such-property".to_string(),
        ),
    ];
    for (case, label) in rows {
        let (status, body) =
            update_property(&router, &asset_id, &label, json!({"propertyAlias": "/plant/3/x"})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // The foreign property is a property of ITS asset, so the same pair is served
    // there: the refusal above is about the asset, not about the property id.
    let (status, answer) = update_property(
        &router,
        &other_asset,
        &foreign.label,
        json!({"propertyAlias": "/plant/3/other"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the property's own asset must serve the write, got {status} {answer}"
    );
}
