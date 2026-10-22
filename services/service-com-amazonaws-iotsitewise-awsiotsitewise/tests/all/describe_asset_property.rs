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
