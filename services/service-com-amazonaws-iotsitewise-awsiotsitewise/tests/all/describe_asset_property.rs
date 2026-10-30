// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeAssetProperty`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: `DescribeAssetProperty` REPORTS the stored asset
//! property the two `CustomID` label forms name — its id, external id, name, data
//! type, unit, alias and notification as the store holds them, and its property
//! type as the referenced asset model's definition declares it now — under
//! `assetProperty` for a property of the asset itself and under `compositeModel`
//! for one a composite model of its model declares, and refuses a label that names
//! no stored asset or property with the modelled 404 instead of answering.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the asset/model fixtures this crate's other suites already
// have: one router, one `call`, one `property_catalog`.
use super::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_catalog, property_id, router,
};
// The documented notification-topic oracle this crate already has, so both
// property suites check the topic against ONE hand-written form.
use super::asset_property_update::expected_topic;

/// A `CustomID`-shaped id no create ever minted, for the refusal cases.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// `DescribeAssetProperty` on `(asset_label, property_label)`.
async fn describe_property(
    router: &MultiServiceRouter,
    asset_label: &str,
    property_label: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{asset_label}/properties/{property_label}"),
        None,
    )
    .await
}

/// `DescribeAsset`'s view of `asset_id`.
async fn describe_asset(router: &MultiServiceRouter, asset_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {asset_id}: got {status} {body}");
    body
}

/// `DescribeAssetModel`'s view of `model_id`.
async fn describe_model(router: &MultiServiceRouter, model_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel {model_id}: got {status} {body}"
    );
    body
}

/// One row of the reporting table: the property to describe and what the stored
/// definition makes it report.
struct ReportCase {
    case: &'static str,
    /// The name the shared property catalog gives this definition.
    property: &'static str,
    data_type: &'static str,
    unit: Option<&'static str>,
    /// The single `PropertyType` member the definition carries.
    type_key: &'static str,
    /// The attribute default value the model declares, for an attribute row.
    default_value: Option<&'static str>,
}

#[tokio::test]
async fn describe_asset_property_reports_the_stored_property_of_every_catalog_type() {
    let router = router();
    let model_id = create_model(&router, "ReportBoiler", property_catalog()).await;
    let asset_id = create_asset(&router, "ReportBoiler-1", &model_id).await;
    let asset = describe_asset(&router, &asset_id).await;

    // One row per property the shared catalog defines, so the rows differ along
    // the data type (DOUBLE, INTEGER, STRING, BOOLEAN), the presence of a unit and
    // the property type (measurement, attribute).
    let rows = [
        ReportCase {
            case: "DOUBLE measurement with a unit",
            property: "Temperature",
            data_type: "DOUBLE",
            unit: Some("Celsius"),
            type_key: "measurement",
            default_value: None,
        },
        ReportCase {
            case: "INTEGER measurement with a unit",
            property: "Cycles",
            data_type: "INTEGER",
            unit: Some("Count"),
            type_key: "measurement",
            default_value: None,
        },
        ReportCase {
            case: "STRING measurement without a unit",
            property: "Status",
            data_type: "STRING",
            unit: None,
            type_key: "measurement",
            default_value: None,
        },
        ReportCase {
            case: "BOOLEAN measurement without a unit",
            property: "Running",
            data_type: "BOOLEAN",
            unit: None,
            type_key: "measurement",
            default_value: None,
        },
        ReportCase {
            case: "STRING attribute carrying the model's default value",
            property: "Serial",
            data_type: "STRING",
            unit: None,
            type_key: "attribute",
            default_value: Some("unset"),
        },
    ];

    for row in rows {
        let ReportCase {
            case,
            property: name,
            data_type,
            unit,
            type_key,
            default_value,
        } = row;
        let id = property_id(&asset["assetProperties"], name);
        let (status, body) = describe_property(&router, &asset_id, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {body}"
        );

        // The asset the property belongs to, reported from the same stored record
        // `DescribeAsset` answers from.
        assert_eq!(
            body["assetId"].as_str(),
            Some(asset_id.as_str()),
            "case {case:?}: expected assetId {asset_id}, got {body}"
        );
        assert_eq!(
            body["assetName"].as_str(),
            Some("ReportBoiler-1"),
            "case {case:?}: expected assetName ReportBoiler-1, got {body}"
        );
        assert_eq!(
            body["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {case:?}: expected assetModelId {model_id}, got {body}"
        );
        assert!(
            body.get("assetExternalId").is_none_or(Value::is_null),
            "case {case:?}: an asset created without an external id must report none, got {body}"
        );

        // A property of the asset itself is reported under `assetProperty`, never
        // as a composite model's.
        let property = &body["assetProperty"];
        assert!(
            body.get("compositeModel").is_none_or(Value::is_null),
            "case {case:?}: a top-level property carries no compositeModel, got {body}"
        );
        assert_eq!(
            property["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the addressed property id {id}, got {property}"
        );
        assert_eq!(
            property["name"].as_str(),
            Some(name),
            "case {case:?}: expected name {name:?}, got {property}"
        );
        assert_eq!(
            property["dataType"].as_str(),
            Some(data_type),
            "case {case:?}: expected dataType {data_type:?}, got {property}"
        );
        assert_eq!(
            property.get("unit").and_then(Value::as_str),
            unit,
            "case {case:?}: expected unit {unit:?}, got {property}"
        );
        // A freshly created asset's property carries neither an alias nor a
        // notification until `UpdateAssetProperty` writes them.
        assert!(
            property.get("alias").is_none_or(Value::is_null),
            "case {case:?}: expected no alias, got {property}"
        );
        assert!(
            property.get("notification").is_none_or(Value::is_null),
            "case {case:?}: expected no notification, got {property}"
        );

        // The property type comes from the model definition, and a definition
        // carries exactly one type member.
        let property_type = &property["type"];
        assert!(
            property_type[type_key].is_object(),
            "case {case:?}: expected a {type_key} type, got {property_type}"
        );
        assert_eq!(
            property_type
                .as_object()
                .map(|members| members.len())
                .unwrap_or_default(),
            1,
            "case {case:?}: a property carries one type, got {property_type}"
        );
        assert_eq!(
            property_type[type_key]
                .get("defaultValue")
                .and_then(Value::as_str),
            default_value,
            "case {case:?}: expected defaultValue {default_value:?}, got {property_type}"
        );

        // A read reports the same thing twice: describing a property changes
        // nothing a later caller sees.
        let (status, again) = describe_property(&router, &asset_id, &id).await;
        assert_eq!(status, 200, "case {case:?}: second read got {status} {again}");
        assert_eq!(
            again, body,
            "case {case:?}: a read must not change what a later read reports"
        );
    }
}

#[tokio::test]
async fn describe_asset_property_reports_the_alias_and_notification_the_store_holds() {
    let router = router();
    let model_id = create_model(&router, "AliasReportBoiler", property_catalog()).await;
    let asset_id = create_asset(&router, "AliasReportBoiler-1", &model_id).await;
    let asset = describe_asset(&router, &asset_id).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");

    // Each row writes the property through `UpdateAssetProperty` and then asks
    // `DescribeAssetProperty` what the store holds: the alias (set, rebound,
    // removed), the notification state (ENABLED, DISABLED, defaulted) and the unit
    // (overridden, inherited from the model definition).
    let rows = [
        (
            "alias + ENABLED + unit",
            json!({
                "propertyAlias": "/plant/7/boiler/temperature",
                "propertyNotificationState": "ENABLED",
                "propertyUnit": "Kelvin",
            }),
            Some("/plant/7/boiler/temperature"),
            "ENABLED",
            Some("Kelvin"),
        ),
        (
            "alias rebound, state defaulted, unit inherited",
            json!({"propertyAlias": "/plant/7/boiler/temperature-2"}),
            Some("/plant/7/boiler/temperature-2"),
            "DISABLED",
            Some("Celsius"),
        ),
        (
            "every optional member omitted",
            json!({}),
            None,
            "DISABLED",
            Some("Celsius"),
        ),
    ];

    for (case, request, alias, notification_state, unit) in rows {
        let (status, answer) = call(
            &router,
            "PUT",
            &format!("/assets/{asset_id}/properties/{temperature}"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAssetProperty got {status} {answer}"
        );

        let (status, body) = describe_property(&router, &asset_id, &temperature).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {body}"
        );
        let property = &body["assetProperty"];
        assert_eq!(
            property.get("alias").and_then(Value::as_str),
            alias,
            "case {case:?}: expected alias {alias:?}, got {property}"
        );
        assert_eq!(
            property["notification"]["state"].as_str(),
            Some(notification_state),
            "case {case:?}: expected notification state {notification_state:?}, got {property}"
        );
        assert_eq!(
            property["notification"]["topic"].as_str(),
            Some(expected_topic(&model_id, &asset_id, &temperature).as_str()),
            "case {case:?}: unexpected notification topic in {property}"
        );
        assert_eq!(
            property.get("unit").and_then(Value::as_str),
            unit,
            "case {case:?}: expected unit {unit:?}, got {property}"
        );

        // `DescribeAsset`'s view of the same stored property agrees with this one:
        // both report the one record, so they cannot disagree.
        let from_asset = describe_asset(&router, &asset_id).await["assetProperties"]
            .as_array()
            .expect("an asset property list")
            .iter()
            .find(|candidate| candidate["id"] == property["id"])
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the property vanished from DescribeAsset"));
        for member in ["alias", "notification", "unit", "dataType", "name"] {
            assert_eq!(
                from_asset.get(member),
                property.get(member),
                "case {case:?}: DescribeAsset and DescribeAssetProperty disagree on {member}: \
                 {from_asset} vs {property}"
            );
        }
    }
}

#[tokio::test]
async fn describe_asset_property_accepts_both_custom_id_forms() {
    let router = router();
    // A model and an asset that carry external ids, plus a property definition
    // with one, so each `CustomID` label form has something to resolve to.
    let model_id = create_model(
        &router,
        "ExternalIdDescribeBoiler",
        json!([
            {"name": "Flow", "dataType": "DOUBLE", "unit": "m3/s",
             "externalId": "flow-describe-definition", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let (status, created) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "ExternalIdDescribeBoiler-1",
            "assetModelId": model_id,
            "assetExternalId": "boiler-describe-external-1",
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created}");
    let asset_id = created["assetId"].as_str().expect("assetId").to_string();
    let flow = property_id(
        &describe_asset(&router, &asset_id).await["assetProperties"],
        "Flow",
    );

    // Each row names the same property through a different pair of label forms, so
    // every row must answer the SAME body.
    let rows = [
        (
            "raw asset id, raw property id",
            asset_id.clone(),
            flow.clone(),
        ),
        (
            "external asset id, raw property id",
            "externalId:boiler-describe-external-1".to_string(),
            flow.clone(),
        ),
        (
            "raw asset id, external property id",
            asset_id.clone(),
            "externalId:flow-describe-definition".to_string(),
        ),
        (
            "both external",
            "externalId:boiler-describe-external-1".to_string(),
            "externalId:flow-describe-definition".to_string(),
        ),
    ];
    let mut answers: Vec<(&str, Value)> = Vec::new();
    for (case, asset_label, property_label) in rows {
        let (status, body) = describe_property(&router, &asset_label, &property_label).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {body}"
        );
        assert_eq!(
            body["assetProperty"]["id"].as_str(),
            Some(flow.as_str()),
            "case {case:?}: a label form must not change the property id, got {body}"
        );
        assert_eq!(
            body["assetProperty"]["externalId"].as_str(),
            Some("flow-describe-definition"),
            "case {case:?}: the property's external id must be reported, got {body}"
        );
        assert_eq!(
            body["assetExternalId"].as_str(),
            Some("boiler-describe-external-1"),
            "case {case:?}: the asset's external id must be reported, got {body}"
        );
        answers.push((case, body));
    }
    let (first_case, first) = answers.first().expect("one answer per row").clone();
    for (case, body) in &answers {
        assert_eq!(
            body, &first,
            "case {case:?}: must answer exactly what {first_case:?} answered"
        );
    }
}

#[tokio::test]
async fn a_property_of_a_composite_model_is_reported_as_a_composite_model_property() {
    let router = router();
    // Two composite models, so the answer has to name the one that declares the
    // property rather than the first stored one.
    let (status, created) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "CompositeDescribeBoiler",
            "assetModelProperties": property_catalog(),
            "assetModelCompositeModels": [
                {"name": "Alarm", "type": "AWS/ALARM", "properties": [
                    {"name": "AlarmState", "dataType": "STRING", "type": {"measurement": {}}},
                ]},
                {"name": "Shift", "type": "CUSTOM", "externalId": "shift-composite",
                 "properties": [
                    {"name": "Operator", "dataType": "STRING",
                     "type": {"attribute": {"defaultValue": "unassigned"}}},
                    {"name": "Throughput", "dataType": "DOUBLE", "unit": "units/h",
                     "type": {"measurement": {}}},
                ]},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .expect("assetModelId")
        .to_string();
    let asset_id = create_asset(&router, "CompositeDescribeBoiler-1", &model_id).await;

    // The composite models as the model reports them: the test reads the ids it
    // addresses from the service rather than assuming any.
    let model = describe_model(&router, &model_id).await;
    let composites = model["assetModelCompositeModels"]
        .as_array()
        .unwrap_or_else(|| panic!("no composite models in {model}"))
        .clone();
    let composite_by_name = |name: &str| -> Value {
        composites
            .iter()
            .find(|composite| composite["name"] == name)
            .unwrap_or_else(|| panic!("no composite model named {name} in {model}"))
            .clone()
    };

    // One row per composite property: two composite models, two property types and
    // a definition with and without a unit.
    let rows = [
        (
            "measurement of the alarm composite model",
            "Alarm",
            "AWS/ALARM",
            "AlarmState",
            "STRING",
            None,
            "measurement",
            None,
        ),
        (
            "attribute of the custom composite model",
            "Shift",
            "CUSTOM",
            "Operator",
            "STRING",
            None,
            "attribute",
            Some("unassigned"),
        ),
        (
            "measurement with a unit of the custom composite model",
            "Shift",
            "CUSTOM",
            "Throughput",
            "DOUBLE",
            Some("units/h"),
            "measurement",
            None,
        ),
    ];
    for (case, composite_name, composite_type, name, data_type, unit, type_key, default_value) in
        rows
    {
        let composite = composite_by_name(composite_name);
        let id = property_id(&composite["properties"], name);
        let (status, body) = describe_property(&router, &asset_id, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {body}"
        );
        assert!(
            body.get("assetProperty").is_none_or(Value::is_null),
            "case {case:?}: a composite model's property is not reported as the asset's, got {body}"
        );
        let reported = &body["compositeModel"];
        assert_eq!(
            reported["name"].as_str(),
            Some(composite_name),
            "case {case:?}: expected composite model {composite_name:?}, got {reported}"
        );
        assert_eq!(
            reported["type"].as_str(),
            Some(composite_type),
            "case {case:?}: expected composite model type {composite_type:?}, got {reported}"
        );
        assert_eq!(
            reported["id"].as_str(),
            composite["id"].as_str(),
            "case {case:?}: expected the declaring composite model's id, got {reported}"
        );
        assert_eq!(
            reported["externalId"].as_str(),
            composite.get("externalId").and_then(Value::as_str),
            "case {case:?}: expected the declaring composite model's external id, got {reported}"
        );
        let property = &reported["assetProperty"];
        assert_eq!(
            property["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the addressed property id {id}, got {property}"
        );
        assert_eq!(
            property["name"].as_str(),
            Some(name),
            "case {case:?}: expected name {name:?}, got {property}"
        );
        assert_eq!(
            property["dataType"].as_str(),
            Some(data_type),
            "case {case:?}: expected dataType {data_type:?}, got {property}"
        );
        assert_eq!(
            property.get("unit").and_then(Value::as_str),
            unit,
            "case {case:?}: expected unit {unit:?}, got {property}"
        );
        assert_eq!(
            property["type"][type_key]
                .get("defaultValue")
                .and_then(Value::as_str),
            default_value,
            "case {case:?}: expected defaultValue {default_value:?}, got {property}"
        );
    }

    // The asset's own properties still answer under `assetProperty`, so the two
    // output members are decided by WHICH model declares the property.
    let temperature = property_id(
        &describe_asset(&router, &asset_id).await["assetProperties"],
        "Temperature",
    );
    let (status, body) = describe_property(&router, &asset_id, &temperature).await;
    assert_eq!(status, 200, "a top-level property: got {status} {body}");
    assert!(
        body["assetProperty"]["id"].as_str() == Some(temperature.as_str())
            && body.get("compositeModel").is_none_or(Value::is_null),
        "a top-level property must answer under assetProperty alone, got {body}"
    );
}

/// Create an asset model of `model_type` carrying `properties`, and return its id.
///
/// Shared with the asset-delete suite (`asset_delete_data_segments`), which needs
/// a `COMPONENT_MODEL` to drive the reuse path, rather than growing a second copy.
pub(crate) async fn create_typed_model(
    router: &MultiServiceRouter,
    name: &str,
    model_type: &str,
    properties: Value,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelType": model_type,
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(status, 202, "create {model_type} {name}: got {status} {body}");
    body["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create {model_type} {name}: no assetModelId in {body}"))
        .to_string()
}

/// Add a composite model described by `request` to `model_id`, and return its id.
///
/// Shared with the asset-delete suite, for the reason [`create_typed_model`] is.
pub(crate) async fn add_composite_model(
    router: &MultiServiceRouter,
    model_id: &str,
    request: Value,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/asset-models/{model_id}/composite-models"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModelCompositeModel {request}: got {status} {body}"
    );
    body["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelId in {body}"))
        .to_string()
}

/// `DescribeAssetModelCompositeModel`'s view of one composite model, which is
/// where the test reads the property ids a composite model contributes.
///
/// Shared with the asset-delete suite, for the reason [`create_typed_model`] is.
pub(crate) async fn describe_model_composite(
    router: &MultiServiceRouter,
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
        "DescribeAssetModelCompositeModel {composite_id}: got {status} {body}"
    );
    body
}

/// One row of the composite-model table: a property one of the host model's
/// composite models contributes, and what `DescribeAssetProperty` must report.
struct CompositeCase {
    case: &'static str,
    /// The composite model on the host model that contributes the property.
    composite: &'static str,
    composite_type: &'static str,
    /// The composite model's external id, when it was created with one.
    composite_external_id: Option<&'static str>,
    property: &'static str,
    data_type: &'static str,
    unit: Option<&'static str>,
    type_key: &'static str,
    default_value: Option<&'static str>,
    /// The property definition's external id, when it declares one: the second
    /// `CustomID` label form the same property is addressable by.
    property_external_id: Option<&'static str>,
}

#[tokio::test]
async fn a_property_a_composite_model_contributes_is_described_however_it_declares_it() {
    let router = router();

    // Two `COMPONENT_MODEL`s with properties of their own: a component-model-based
    // composite model stores none, so every property it contributes comes from the
    // component model it reuses.
    let speed_component = create_typed_model(
        &router,
        "SpeedComponent",
        "COMPONENT_MODEL",
        json!([
            {"name": "Speed", "dataType": "DOUBLE", "unit": "m/s", "type": {"measurement": {}}},
            {"name": "Mode", "dataType": "STRING", "externalId": "component-mode-definition",
             "type": {"attribute": {"defaultValue": "idle"}}},
            {"name": "Revolutions", "dataType": "INTEGER", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let pump_component = create_typed_model(
        &router,
        "PumpComponent",
        "COMPONENT_MODEL",
        json!([
            {"name": "Pressure", "dataType": "DOUBLE", "unit": "Bar", "type": {"measurement": {}}},
        ]),
    )
    .await;

    // The host model: its own properties, plus three composite models that differ
    // along WHERE their properties come from — two reusing a component model and
    // one carrying inline definitions.
    let model_id = create_typed_model(
        &router,
        "ComponentHostBoiler",
        "ASSET_MODEL",
        property_catalog(),
    )
    .await;
    let composite_ids = [
        (
            "Reused",
            add_composite_model(
                &router,
                &model_id,
                json!({
                    "assetModelCompositeModelName": "Reused",
                    "assetModelCompositeModelType": "CUSTOM",
                    "composedAssetModelId": &speed_component,
                }),
            )
            .await,
        ),
        (
            "Pump",
            add_composite_model(
                &router,
                &model_id,
                json!({
                    "assetModelCompositeModelName": "Pump",
                    "assetModelCompositeModelType": "CUSTOM",
                    "assetModelCompositeModelExternalId": "pump-composite",
                    "composedAssetModelId": &pump_component,
                }),
            )
            .await,
        ),
        (
            "Inline",
            add_composite_model(
                &router,
                &model_id,
                json!({
                    "assetModelCompositeModelName": "Inline",
                    "assetModelCompositeModelType": "CUSTOM",
                    "assetModelCompositeModelProperties": [
                        {"name": "Operator", "dataType": "STRING",
                         "externalId": "inline-operator-definition",
                         "type": {"attribute": {"defaultValue": "unassigned"}}},
                    ],
                }),
            )
            .await,
        ),
    ];
    let composite_id_of = |name: &str| -> String {
        composite_ids
            .iter()
            .find(|(composite, _)| *composite == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("no composite model named {name} was created"))
    };

    let asset_id = create_asset(&router, "ComponentHostBoiler-1", &model_id).await;

    // One row per property a composite model contributes, so the rows differ along
    // the data type, the presence of a unit, the property type, whether the
    // definition carries an external id, and — the dimension that matters here —
    // whether the declaring composite model stores its own definitions or reuses a
    // component model's.
    let rows = [
        CompositeCase {
            case: "component model's DOUBLE measurement with a unit",
            composite: "Reused",
            composite_type: "CUSTOM",
            composite_external_id: None,
            property: "Speed",
            data_type: "DOUBLE",
            unit: Some("m/s"),
            type_key: "measurement",
            default_value: None,
            property_external_id: None,
        },
        CompositeCase {
            case: "component model's STRING attribute with an external id",
            composite: "Reused",
            composite_type: "CUSTOM",
            composite_external_id: None,
            property: "Mode",
            data_type: "STRING",
            unit: None,
            type_key: "attribute",
            default_value: Some("idle"),
            property_external_id: Some("component-mode-definition"),
        },
        CompositeCase {
            case: "component model's INTEGER measurement without a unit",
            composite: "Reused",
            composite_type: "CUSTOM",
            composite_external_id: None,
            property: "Revolutions",
            data_type: "INTEGER",
            unit: None,
            type_key: "measurement",
            default_value: None,
            property_external_id: None,
        },
        CompositeCase {
            case: "a second component model, reused by a composite model with an external id",
            composite: "Pump",
            composite_type: "CUSTOM",
            composite_external_id: Some("pump-composite"),
            property: "Pressure",
            data_type: "DOUBLE",
            unit: Some("Bar"),
            type_key: "measurement",
            default_value: None,
            property_external_id: None,
        },
        CompositeCase {
            case: "a composite model's own inline attribute definition",
            composite: "Inline",
            composite_type: "CUSTOM",
            composite_external_id: None,
            property: "Operator",
            data_type: "STRING",
            unit: None,
            type_key: "attribute",
            default_value: Some("unassigned"),
            property_external_id: Some("inline-operator-definition"),
        },
    ];

    // The properties `ListAssetProperties` reports for this asset, which must agree
    // with what `DescribeAssetProperty` answers for each of them.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssetProperties: got {status} {listed}");
    let summaries = listed["assetPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetPropertySummaries in {listed}"))
        .clone();

    for row in rows {
        let CompositeCase {
            case,
            composite,
            composite_type,
            composite_external_id,
            property: name,
            data_type,
            unit,
            type_key,
            default_value,
            property_external_id,
        } = row;
        let composite_id = composite_id_of(composite);
        // The property's id is read from the service's own view of the composite
        // model, so the test addresses the id the store actually minted.
        let described_composite = describe_model_composite(&router, &model_id, &composite_id).await;
        let id = property_id(
            &described_composite["assetModelCompositeModelProperties"],
            name,
        );

        let (status, body) = describe_property(&router, &asset_id, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {body}"
        );
        assert!(
            body.get("assetProperty").is_none_or(Value::is_null),
            "case {case:?}: a composite model's property is not reported as the asset's, got {body}"
        );
        assert_eq!(
            body["assetId"].as_str(),
            Some(asset_id.as_str()),
            "case {case:?}: expected assetId {asset_id}, got {body}"
        );
        assert_eq!(
            body["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {case:?}: expected assetModelId {model_id}, got {body}"
        );
        assert_eq!(
            body["assetName"].as_str(),
            Some("ComponentHostBoiler-1"),
            "case {case:?}: expected assetName ComponentHostBoiler-1, got {body}"
        );

        let reported = &body["compositeModel"];
        assert_eq!(
            reported["name"].as_str(),
            Some(composite),
            "case {case:?}: expected composite model {composite:?}, got {reported}"
        );
        assert_eq!(
            reported["type"].as_str(),
            Some(composite_type),
            "case {case:?}: expected composite model type {composite_type:?}, got {reported}"
        );
        assert_eq!(
            reported["id"].as_str(),
            Some(composite_id.as_str()),
            "case {case:?}: expected the declaring composite model's id {composite_id}, \
             got {reported}"
        );
        assert_eq!(
            reported.get("externalId").and_then(Value::as_str),
            composite_external_id,
            "case {case:?}: expected composite external id {composite_external_id:?}, \
             got {reported}"
        );

        let property = &reported["assetProperty"];
        assert_eq!(
            property["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the addressed property id {id}, got {property}"
        );
        assert_eq!(
            property["name"].as_str(),
            Some(name),
            "case {case:?}: expected name {name:?}, got {property}"
        );
        assert_eq!(
            property["dataType"].as_str(),
            Some(data_type),
            "case {case:?}: expected dataType {data_type:?}, got {property}"
        );
        assert_eq!(
            property.get("unit").and_then(Value::as_str),
            unit,
            "case {case:?}: expected unit {unit:?}, got {property}"
        );
        assert_eq!(
            property.get("externalId").and_then(Value::as_str),
            property_external_id,
            "case {case:?}: expected property external id {property_external_id:?}, got {property}"
        );
        assert_eq!(
            property["type"][type_key]
                .get("defaultValue")
                .and_then(Value::as_str),
            default_value,
            "case {case:?}: expected defaultValue {default_value:?}, got {property}"
        );

        // The same property is listed by `ListAssetProperties` under the same
        // composite model: the two views answer from one definition of what a
        // composite model contributes, so they cannot disagree.
        let summary = summaries
            .iter()
            .find(|summary| summary["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: property {id} is not in {listed}")
            });
        assert_eq!(
            summary["assetCompositeModelId"].as_str(),
            Some(composite_id.as_str()),
            "case {case:?}: ListAssetProperties and DescribeAssetProperty disagree on the \
             declaring composite model: {summary} vs {reported}"
        );

        // Both `CustomID` label forms name the same property, so a definition with
        // an external id answers the same body through either.
        if let Some(external) = property_external_id {
            let (status, by_external) =
                describe_property(&router, &asset_id, &format!("externalId:{external}")).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the externalId label got {status} {by_external}"
            );
            assert_eq!(
                by_external, body,
                "case {case:?}: the externalId label must answer what the raw id answered"
            );
        }

        // Describing a property reports the same thing twice: a read changes
        // nothing a later caller sees.
        let (status, again) = describe_property(&router, &asset_id, &id).await;
        assert_eq!(status, 200, "case {case:?}: second read got {status} {again}");
        assert_eq!(
            again, body,
            "case {case:?}: a read must not change what a later read reports"
        );
    }

    // A property added to the reused component model AFTER the composite model was
    // created is contributed too: what a composite model contributes is read from
    // the component model's stored record, not copied when the composite model is
    // created.
    let component = describe_model(&router, &speed_component).await;
    let mut definitions = component["assetModelProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelProperties in {component}"))
        .clone();
    definitions.push(json!({
        "name": "Torque", "dataType": "DOUBLE", "unit": "Nm", "type": {"measurement": {}},
    }));
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/asset-models/{speed_component}"),
        Some(json!({
            "assetModelName": "SpeedComponent",
            "assetModelProperties": definitions,
        })),
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {updated}");

    let reused_id = composite_id_of("Reused");
    let added = property_id(
        &describe_model_composite(&router, &model_id, &reused_id).await
            ["assetModelCompositeModelProperties"],
        "Torque",
    );
    let (status, body) = describe_property(&router, &asset_id, &added).await;
    assert_eq!(
        status, 200,
        "a property added to the component model: got {status} {body}"
    );
    assert_eq!(
        body["compositeModel"]["id"].as_str(),
        Some(reused_id.as_str()),
        "a property added to the component model must be reported under the composite model \
         that reuses it, got {body}"
    );
    assert_eq!(
        body["compositeModel"]["assetProperty"]["unit"].as_str(),
        Some("Nm"),
        "expected the added definition's unit Nm, got {body}"
    );
}

#[tokio::test]
async fn a_label_that_names_no_stored_property_is_a_modelled_refusal() {
    let router = router();
    let model_id = create_model(&router, "RefusalDescribeBoiler", property_catalog()).await;
    let asset_id = create_asset(&router, "RefusalDescribeBoiler-1", &model_id).await;
    let temperature = property_id(
        &describe_asset(&router, &asset_id).await["assetProperties"],
        "Temperature",
    );
    // A second asset of ANOTHER model, so a property id that exists elsewhere is
    // still not this asset's.
    let other_model = create_model(
        &router,
        "RefusalDescribeOther",
        json!([{"name": "Pressure", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let other_asset = create_asset(&router, "RefusalDescribeOther-1", &other_model).await;
    let pressure = property_id(
        &describe_asset(&router, &other_asset).await["assetProperties"],
        "Pressure",
    );

    let rows = [
        (
            "unknown asset id",
            ABSENT_ID.to_string(),
            temperature.clone(),
        ),
        (
            "unknown property id",
            asset_id.clone(),
            ABSENT_ID.to_string(),
        ),
        (
            "unknown asset external id",
            "externalId:no-such-asset".to_string(),
            temperature.clone(),
        ),
        (
            "unknown property external id",
            asset_id.clone(),
            "externalId:no-such-property".to_string(),
        ),
        (
            "a property of another asset's model",
            asset_id.clone(),
            pressure.clone(),
        ),
        (
            "an asset model id in the asset position",
            model_id.clone(),
            temperature.clone(),
        ),
    ];
    for (case, asset_label, property_label) in rows {
        let (status, body) = describe_property(&router, &asset_label, &property_label).await;
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
}
