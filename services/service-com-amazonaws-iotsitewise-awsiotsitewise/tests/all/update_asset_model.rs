// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateAssetModel`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rules these suites pin, each over a SET of inputs rather than one recorded
//! request:
//!
//! 1. The request carries the model's NEW definition in full: after the call the
//!    stored model is exactly what the request described — name, description,
//!    property list, hierarchy list — and anything the request left out is gone.
//!    A member of the definition the request still names keeps its stored id, so a
//!    retained property stays the same property.
//! 2. An existing property may change its name, unit and the inside of its type,
//!    but not its property type or its data type: "You can’t change the type or
//!    data type of an existing property" — refused `400 InvalidRequestException`,
//!    with the stored definition untouched.
//! 3. The conditional headers are evaluated BEFORE anything is written: a failed
//!    `If-Match` / `If-None-Match` precondition is `412 PreconditionFailedException`
//!    and the model keeps its definition and its ETag.
//! 4. Every asset created from the model inherits the updated definition: a
//!    property the model no longer defines goes from the asset, and its stored data
//!    goes with it; a property the model gained appears on the asset.
//! 5. A definition past a published per-model quota is refused
//!    `410 LimitExceededException`.
//! 6. An id — of the model itself or of a hierarchy's child model — that resolves
//!    to no stored asset model is `404 ResourceNotFoundException`.
//!
//! The wire harness (`router`, `call`, `create_model`, …) lives in the
//! `asset_lifecycle` suite and the ETag helpers in `hierarchies_etags_and_refusals`;
//! this file reuses both rather than keeping a third copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, is_modelled_id, router, router_with_engine,
};
use crate::hierarchies_etags_and_refusals::{
    call_with_headers, describe_e_tag, is_modelled_e_tag, FOREIGN_E_TAG, PAST_ACTIVATION,
};

/// Where `BatchPutAssetPropertyValue` is bound, used here to give a property the
/// stored data that removing it from the model must delete.
const BATCH_PUT: &str = "/properties";

/// A valid `CustomID` UUID no create in this suite ever mints.
const UNKNOWN_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The properties every model in this suite starts from: one measurement and one
/// attribute, so a row that keeps one and drops the other is a different input.
fn base_properties() -> Value {
    json!([
        {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}},
        {"name": "Serial", "dataType": "STRING", "type": {"attribute": {"defaultValue": "unset"}}},
    ])
}

/// `PUT /asset-models/{assetModelId}` with `body` and the extra request headers
/// `headers` carries.
async fn update_model(
    router: &MultiServiceRouter,
    model_id: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let (status, _, answered) = call_with_headers(
        router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(body),
        headers,
    )
    .await;
    (status, answered)
}

/// `POST /asset-models` for a model carrying a description and
/// [`base_properties`], answering its id.
async fn create_described_model(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelDescription": "the definition before the update",
            "assetModelProperties": base_properties(),
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel {name}: got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel {name}: no assetModelId in {created}"))
        .to_string()
}

/// `GET /asset-models/{assetModelId}`, asserting it answered.
async fn describe_model(router: &MultiServiceRouter, model_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel {model_id}: got {status} {body}");
    body
}

/// The model's property definitions as `DescribeAssetModel` reports them — each
/// carrying its stored id, which is how an update KEEPS a property: an id-less
/// definition whose name the model already holds asks for the replacement the
/// service refuses.
async fn stored_properties(router: &MultiServiceRouter, model_id: &str) -> Value {
    describe_model(router, model_id).await["assetModelProperties"].clone()
}

/// `GET /assets/{assetId}`, asserting it answered.
async fn describe_asset(router: &MultiServiceRouter, asset_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {asset_id}: got {status} {body}");
    body
}

/// The `name` of every element of a described definition list, in order.
fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|list| {
            list.iter()
                .map(|member| member["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `name` -> `id` pairs of a described definition list.
fn ids_by_name(list: &Value) -> Vec<(String, String)> {
    list.as_array()
        .map(|list| {
            list.iter()
                .map(|member| {
                    (
                        member["name"].as_str().unwrap_or_default().to_string(),
                        member["id"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The id stored against `name`, if the list holds it.
fn id_of(pairs: &[(String, String)], name: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(held, _)| held == name)
        .map(|(_, id)| id.clone())
}

/// One row of the replacement table: what the update asks for, and the definition
/// the model must then report.
struct ReplacementRow {
    case: &'static str,
    /// Built from the model as `DescribeAssetModel` reports it before the update,
    /// so a row can resubmit the stored properties with their ids.
    update: Box<dyn Fn(&Value) -> Value>,
    expected_name: Option<&'static str>,
    expected_description: &'static str,
    expected_properties: Vec<&'static str>,
    expected_hierarchies: Vec<&'static str>,
    expected_external_id: Option<&'static str>,
}

#[tokio::test]
async fn update_asset_model_replaces_the_whole_stored_definition() {
    let router = router();
    // The model a hierarchy row points at; it must exist for the hierarchy to be
    // accepted at all.
    let child_model = create_model(&router, "Sensor", json!([])).await;
    let child_for_rows = child_model.clone();

    // Rows differ along what the replacement rule generalises over: which member of
    // the definition the request changes (name, description, properties,
    // hierarchies, external id), and in which direction (kept, added, removed,
    // edited, all removed).
    let rows: Vec<ReplacementRow> = vec![
        ReplacementRow {
            case: "the name alone, properties resubmitted",
            update: Box::new(|before| {
                json!({
                    "assetModelName": "Renamed",
                    "assetModelProperties": before["assetModelProperties"].clone(),
                })
            }),
            expected_name: Some("Renamed"),
            // The request carried no description, so the model keeps none: this
            // request describes the new state rather than patching the old one.
            expected_description: "",
            expected_properties: vec!["Temperature", "Serial"],
            expected_hierarchies: vec![],
            expected_external_id: None,
        },
        ReplacementRow {
            case: "a new description",
            update: Box::new(|before| {
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelDescription": "the definition after the update",
                    "assetModelProperties": before["assetModelProperties"].clone(),
                })
            }),
            expected_name: None,
            expected_description: "the definition after the update",
            expected_properties: vec!["Temperature", "Serial"],
            expected_hierarchies: vec![],
            expected_external_id: None,
        },
        ReplacementRow {
            case: "a property added",
            update: Box::new(|before| {
                let mut properties = before["assetModelProperties"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                properties.push(json!({
                    "name": "Pressure", "dataType": "INTEGER", "unit": "kPa",
                    "type": {"measurement": {}},
                }));
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": properties,
                })
            }),
            expected_name: None,
            expected_description: "",
            expected_properties: vec!["Temperature", "Serial", "Pressure"],
            expected_hierarchies: vec![],
            expected_external_id: None,
        },
        ReplacementRow {
            case: "one property removed",
            update: Box::new(|before| {
                let properties: Vec<Value> = before["assetModelProperties"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|property| property["name"] != "Temperature")
                    .collect();
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": properties,
                })
            }),
            expected_name: None,
            expected_description: "",
            expected_properties: vec!["Serial"],
            expected_hierarchies: vec![],
            expected_external_id: None,
        },
        ReplacementRow {
            case: "every property removed by omitting the list",
            update: Box::new(|before| json!({ "assetModelName": before["assetModelName"].clone() })),
            expected_name: None,
            expected_description: "",
            expected_properties: vec![],
            expected_hierarchies: vec![],
            expected_external_id: None,
        },
        ReplacementRow {
            case: "a property's name and unit edited",
            update: Box::new(|before| {
                let properties: Vec<Value> = before["assetModelProperties"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mut property| {
                        if property["name"] == "Temperature" {
                            property["name"] = json!("CoreTemperature");
                            property["unit"] = json!("Kelvin");
                        }
                        property
                    })
                    .collect();
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": properties,
                })
            }),
            expected_name: None,
            expected_description: "",
            expected_properties: vec!["CoreTemperature", "Serial"],
            expected_hierarchies: vec![],
            expected_external_id: None,
        },
        ReplacementRow {
            case: "a hierarchy added",
            update: Box::new(move |before| {
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": before["assetModelProperties"].clone(),
                    "assetModelHierarchies": [
                        {"name": "Sensors", "childAssetModelId": child_for_rows.clone()},
                    ],
                })
            }),
            expected_name: None,
            expected_description: "",
            expected_properties: vec!["Temperature", "Serial"],
            expected_hierarchies: vec!["Sensors"],
            expected_external_id: None,
        },
        ReplacementRow {
            case: "an external id assigned",
            update: Box::new(|before| {
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelExternalId": "boiler.line-one",
                    "assetModelProperties": before["assetModelProperties"].clone(),
                })
            }),
            expected_name: None,
            expected_description: "",
            expected_properties: vec!["Temperature", "Serial"],
            expected_hierarchies: vec![],
            expected_external_id: Some("boiler.line-one"),
        },
    ];

    // One model per row, all created first and then waited for ONCE: the owed
    // CREATING -> ACTIVE transition is a clock the store shares, so after a single
    // wait every model has settled and the state the update reports is the one
    // THIS call owes. Each carries a description, so a row whose request carries
    // none is observably a removal, and each carries a NAME of its own, because
    // `assetModelName` is unique per account; a row that keeps its name resubmits
    // the one its model reads back with.
    let mut model_ids = Vec::new();
    for at in 0..rows.len() {
        model_ids.push(create_described_model(&router, &format!("Boiler-{at}")).await);
    }
    tokio::time::sleep(PAST_ACTIVATION).await;

    for (row, model_id) in rows.iter().zip(&model_ids) {
        let model_id = model_id.clone();
        let seed = describe_model(&router, &model_id).await;
        assert_eq!(
            seed["assetModelDescription"].as_str(),
            Some("the definition before the update"),
            "case {:?}: the seed model must carry a description, got {seed}",
            row.case
        );
        let before_ids = ids_by_name(&seed["assetModelProperties"]);
        // The name the row's model was created with: what a row that does not
        // rename must still report.
        let seed_name = seed["assetModelName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let before_tag = describe_e_tag(&router, &model_id).await;
        let before_version = seed["assetModelVersion"].as_str().unwrap_or("0").to_string();

        let (status, answered) = update_model(&router, &model_id, (row.update)(&seed), &[]).await;
        assert_eq!(
            status, 202,
            "case {:?}: expected 202, got {status} {answered}",
            row.case
        );
        assert_eq!(
            answered["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {:?}: the answer must name the updated model, got {answered}",
            row.case
        );
        // `assetModelStatus` "contains a state (UPDATING after successfully calling
        // this operation)".
        assert_eq!(
            answered["assetModelStatus"]["state"].as_str(),
            Some("UPDATING"),
            "case {:?}: expected the UPDATING state, got {answered}",
            row.case
        );

        let after = describe_model(&router, &model_id).await;
        assert_eq!(
            after["assetModelName"].as_str(),
            Some(row.expected_name.unwrap_or(&seed_name)),
            "case {:?}: unexpected name in {after}",
            row.case
        );
        assert_eq!(
            after["assetModelDescription"].as_str().unwrap_or_default(),
            row.expected_description,
            "case {:?}: unexpected description in {after}",
            row.case
        );
        assert_eq!(
            names(&after["assetModelProperties"]),
            row.expected_properties,
            "case {:?}: unexpected property list in {after}",
            row.case
        );
        assert_eq!(
            names(&after["assetModelHierarchies"]),
            row.expected_hierarchies,
            "case {:?}: unexpected hierarchy list in {after}",
            row.case
        );
        assert_eq!(
            after["assetModelExternalId"].as_str(),
            row.expected_external_id,
            "case {:?}: unexpected external id in {after}",
            row.case
        );

        // A member the request still names keeps its stored id; a member it adds
        // gets a minted one.
        let after_ids = ids_by_name(&after["assetModelProperties"]);
        for (name, id) in &after_ids {
            match id_of(&before_ids, name) {
                Some(held) => assert_eq!(
                    id, &held,
                    "case {:?}: the retained property {name} must keep its id, got {id}",
                    row.case
                ),
                None => assert!(
                    is_modelled_id(id),
                    "case {:?}: the new property {name} must carry a modelled id, got {id:?}",
                    row.case
                ),
            }
        }
        for (name, id) in &before_ids {
            if row.expected_properties.contains(&name.as_str()) {
                assert_eq!(
                    id_of(&after_ids, name).as_deref(),
                    Some(id.as_str()),
                    "case {:?}: the property {name} must still be stored under {id}",
                    row.case
                );
            }
        }

        // A new definition is a new version, and so a new ETag.
        let after_tag = describe_e_tag(&router, &model_id).await;
        assert!(
            is_modelled_e_tag(&after_tag),
            "case {:?}: the ETag must satisfy ^[\\w-]{{43}}$, got {after_tag:?}",
            row.case
        );
        assert_ne!(
            after_tag, before_tag,
            "case {:?}: the updated definition must report a new ETag, both {after_tag}",
            row.case
        );
        let after_version = after["assetModelVersion"].as_str().unwrap_or("0");
        assert_ne!(
            after_version, before_version,
            "case {:?}: the updated model must report a new version, both {after_version}",
            row.case
        );
    }

    // An assigned external id is addressable: `externalId:<id>` names the model.
    let aliased = describe_model(&router, "externalId:boiler.line-one").await;
    assert_eq!(
        aliased["assetModelExternalId"].as_str(),
        Some("boiler.line-one"),
        "the externalId: label must resolve to the model it was assigned to, got {aliased}"
    );
    // The child model the hierarchy row named is untouched by any of it.
    let child = describe_model(&router, &child_model).await;
    assert_eq!(
        child["assetModelName"].as_str(),
        Some("Sensor"),
        "the child model must be unchanged, got {child}"
    );
}

/// One row of the property-rule table: the edit applied to a described property
/// list, and the status the update must answer.
struct PropertyRuleRow {
    case: &'static str,
    update: Box<dyn Fn(&Value) -> Value>,
    expected_status: u16,
}

#[tokio::test]
async fn an_existing_property_may_change_its_name_but_not_its_type_or_data_type() {
    let router = router();

    // The model every row starts from: a DOUBLE measurement, a STRING attribute and
    // a STRUCT whose `dataTypeSpec` is part of its data type. `PropertyDataType` is
    // STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON and
    // `PropertyType` carries attribute | measurement | transform | metric
    // (`smithy_query(query="shape", shape="PropertyDataType,PropertyType")`), so the
    // rows below move a property along each of those two axes.
    let seed_properties = json!([
        {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}},
        {"name": "Serial", "dataType": "STRING", "type": {"attribute": {"defaultValue": "unset"}}},
        {"name": "AlarmState", "dataType": "STRUCT", "dataTypeSpec": "AWS/ALARM_STATE",
         "type": {"measurement": {}}},
    ]);

    /// The described property list with `name`'s member rewritten by `edit`.
    fn edited(before: &Value, name: &str, edit: impl Fn(&mut Value)) -> Value {
        let properties: Vec<Value> = before["assetModelProperties"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut property| {
                if property["name"] == name {
                    edit(&mut property);
                }
                property
            })
            .collect();
        json!({
            "assetModelName": before["assetModelName"].clone(),
            "assetModelProperties": properties,
        })
    }

    let rows: Vec<PropertyRuleRow> = vec![
        PropertyRuleRow {
            case: "the stored definition resubmitted unchanged",
            update: Box::new(|before| edited(before, "Temperature", |_| {})),
            expected_status: 202,
        },
        PropertyRuleRow {
            case: "an existing property's name and unit changed",
            update: Box::new(|before| {
                edited(before, "Temperature", |property| {
                    property["name"] = json!("CoreTemperature");
                    property["unit"] = json!("Kelvin");
                })
            }),
            expected_status: 202,
        },
        PropertyRuleRow {
            case: "an existing measurement's data type changed",
            update: Box::new(|before| {
                edited(before, "Temperature", |property| {
                    property["dataType"] = json!("INTEGER");
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "an existing attribute's data type changed",
            update: Box::new(|before| {
                edited(before, "Serial", |property| {
                    property["dataType"] = json!("BOOLEAN");
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "an existing measurement turned into an attribute",
            update: Box::new(|before| {
                edited(before, "Temperature", |property| {
                    property["type"] = json!({"attribute": {"defaultValue": "0"}});
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "an existing attribute turned into a measurement",
            update: Box::new(|before| {
                edited(before, "Serial", |property| {
                    property["type"] = json!({"measurement": {}});
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "an existing STRUCT's dataTypeSpec changed",
            update: Box::new(|before| {
                edited(before, "AlarmState", |property| {
                    property["dataTypeSpec"] = json!("AWS/L4E_ANOMALY_RESULT");
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "the same stored property named twice",
            update: Box::new(|before| {
                let mut properties = before["assetModelProperties"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let first = properties.first().cloned().expect("a stored property");
                properties.push(first);
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": properties,
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "a new property carrying no property type",
            update: Box::new(|before| {
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": [{"name": "P", "dataType": "DOUBLE", "type": {}}],
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "a new property carrying two property types",
            update: Box::new(|before| {
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": [{
                        "name": "P", "dataType": "DOUBLE",
                        "type": {"measurement": {}, "attribute": {}},
                    }],
                })
            }),
            expected_status: 400,
        },
        PropertyRuleRow {
            case: "a new STRUCT property carrying no dataTypeSpec",
            update: Box::new(|before| {
                json!({
                    "assetModelName": before["assetModelName"].clone(),
                    "assetModelProperties": [{
                        "name": "P", "dataType": "STRUCT", "type": {"measurement": {}},
                    }],
                })
            }),
            expected_status: 400,
        },
    ];

    for (at, row) in rows.iter().enumerate() {
        // A model of its own per row, under a name of its own: `assetModelName` is
        // unique per account, and every row resubmits the name its model holds.
        let model_id = create_model(&router, &format!("Boiler-{at}"), seed_properties.clone()).await;
        let before = describe_model(&router, &model_id).await;
        let before_tag = describe_e_tag(&router, &model_id).await;

        let (status, answered) = update_model(&router, &model_id, (row.update)(&before), &[]).await;
        assert_eq!(
            status, row.expected_status,
            "case {:?}: expected {}, got {status} {answered}",
            row.case, row.expected_status
        );

        let after = describe_model(&router, &model_id).await;
        let after_tag = describe_e_tag(&router, &model_id).await;
        if row.expected_status == 400 {
            assert_eq!(
                error_code(&answered),
                "InvalidRequestException",
                "case {:?}: unexpected error code in {answered}",
                row.case
            );
            // A refused update changes nothing a later caller sees.
            assert_eq!(
                after_tag, before_tag,
                "case {:?}: a refused update must leave the definition alone, got {after_tag}",
                row.case
            );
            assert_eq!(
                names(&after["assetModelProperties"]),
                names(&before["assetModelProperties"]),
                "case {:?}: a refused update must leave the properties alone, got {after}",
                row.case
            );
        } else {
            assert_ne!(
                after_tag, before_tag,
                "case {:?}: an accepted update must report a new ETag, both {after_tag}",
                row.case
            );
            // An accepted edit keeps the property it edited: its stored id is the
            // one the model held.
            let before_ids = ids_by_name(&before["assetModelProperties"]);
            let after_ids = ids_by_name(&after["assetModelProperties"]);
            assert_eq!(
                after_ids.len(),
                before_ids.len(),
                "case {:?}: expected {} properties, got {after}",
                row.case,
                before_ids.len()
            );
            let before_set: Vec<&String> = before_ids.iter().map(|(_, id)| id).collect();
            for (name, id) in &after_ids {
                assert!(
                    before_set.contains(&id),
                    "case {:?}: the edited property {name} must keep a stored id, got {id}",
                    row.case
                );
            }
        }
    }
}

/// One row of the conditional-header table: the case, the headers it sends (where
/// `$ETAG` stands for the tag the read reported and `$STALE` for the tag the model
/// carried one version ago), and the status `UpdateAssetModel` must answer.
type ConditionalRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16);

/// The same row once the model it runs against has been created: the row, that
/// model's id, and the ETag of the version before its seeding update.
type ConditionalCase<'a> = (&'a str, &'a Vec<(&'a str, &'a str)>, u16, String, String);

#[tokio::test]
async fn update_asset_model_evaluates_its_conditional_headers_before_writing() {
    let router = router();

    // Rows span both headers, both `AssetModelVersionType` values
    // (`smithy_query(query="shape", shape="AssetModelVersionType")`: ACTIVE |
    // LATEST), the absent version type, and a correct, a stale and a foreign
    // `If-Match` token.
    let rows: Vec<ConditionalRow<'_>> = vec![
        ("no conditional header", vec![], 202),
        ("If-Match with the tag the read reported", vec![("if-match", "$ETAG")], 202),
        (
            "If-Match with the tag the read reported, LATEST",
            vec![("if-match", "$ETAG"), ("match-for-version-type", "LATEST")],
            202,
        ),
        (
            "If-Match with the tag the read reported, ACTIVE",
            vec![("if-match", "$ETAG"), ("match-for-version-type", "ACTIVE")],
            202,
        ),
        ("If-Match with a foreign tag", vec![("if-match", FOREIGN_E_TAG)], 412),
        (
            "If-Match with a foreign tag, ACTIVE",
            vec![("if-match", FOREIGN_E_TAG), ("match-for-version-type", "ACTIVE")],
            412,
        ),
        ("If-Match with the tag of an earlier version", vec![("if-match", "$STALE")], 412),
        ("If-None-Match on an existing LATEST version", vec![("if-none-match", "*")], 412),
        (
            "If-None-Match on an existing ACTIVE version",
            vec![("if-none-match", "*"), ("match-for-version-type", "ACTIVE")],
            412,
        ),
    ];

    // One model per row, each under a name of its own because `assetModelName` is
    // unique per account, each already updated once so it has an earlier version
    // whose tag is the `$STALE` token, then waited for ONCE so every model's owed
    // transition has fallen due and its ACTIVE version exists.
    let mut cases: Vec<ConditionalCase<'_>> = Vec::new();
    for (at, (case, headers, expected)) in rows.iter().enumerate() {
        let seed_name = format!("Boiler-{at}");
        let model_id = create_model(&router, &seed_name, base_properties()).await;
        let stale = describe_e_tag(&router, &model_id).await;
        // The seeding update resubmits the stored properties BY THEIR IDS, which
        // is what keeps them: an id-less definition whose name the model already
        // holds asks for a replacement the service refuses.
        let kept = stored_properties(&router, &model_id).await;
        let (status, answered) = update_model(
            &router,
            &model_id,
            json!({"assetModelName": seed_name, "assetModelProperties": kept}),
            &[],
        )
        .await;
        assert_eq!(status, 202, "case {case:?}: seeding update got {status} {answered}");
        cases.push((case, headers, *expected, model_id, stale));
    }
    tokio::time::sleep(PAST_ACTIVATION).await;

    for (case, headers, expected, model_id, stale) in &cases {
        let tag = describe_e_tag(&router, model_id).await;
        // The name this model holds, and the one the conditioned update asks for:
        // a rename may not take a name another model holds, so each row's target
        // is derived from the name its own model carries.
        let held_name = describe_model(&router, model_id).await["assetModelName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let conditioned = format!("{held_name}-conditioned");
        assert_ne!(
            &tag, stale,
            "case {case:?}: the seeding update must have changed the tag, both {tag}"
        );
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| {
                let value = match *value {
                    "$ETAG" => tag.as_str(),
                    "$STALE" => stale.as_str(),
                    other => other,
                };
                (*name, value)
            })
            .collect();

        let (status, answered) = update_model(
            &router,
            model_id,
            json!({
                "assetModelName": conditioned,
                // The properties this update KEEPS, named by their stored ids.
                "assetModelProperties": stored_properties(&router, model_id).await,
            }),
            &headers,
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );

        let after = describe_model(&router, model_id).await;
        if *expected == 412 {
            assert_eq!(
                error_code(&answered),
                "PreconditionFailedException",
                "case {case:?}: unexpected error code in {answered}"
            );
            // A failed precondition writes nothing: the name, the ETag and so the
            // version are the ones the read reported.
            assert_eq!(
                after["assetModelName"].as_str(),
                Some(held_name.as_str()),
                "case {case:?}: a refused update must leave the name alone, got {after}"
            );
            assert_eq!(
                describe_e_tag(&router, model_id).await,
                tag,
                "case {case:?}: a refused update must leave the ETag alone"
            );
        } else {
            assert_eq!(
                after["assetModelName"].as_str(),
                Some(conditioned.as_str()),
                "case {case:?}: the accepted update must have landed, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn the_conditional_headers_target_a_version_that_may_not_exist_yet() {
    let router = router();

    // The rows the settled matrix above cannot carry: a model whose owed
    // CREATING -> ACTIVE transition has NOT fallen due has no ACTIVE version at
    // all, so a header targeting that version is judged against nothing. The
    // same two headers against the same model are the two outcomes: `If-Match`
    // matches no tag (412), while `If-None-Match` asks that the version not
    // exist, which is exactly the state (202).
    //
    // (case, headers, expected status)
    let rows: Vec<ConditionalRow<'_>> = vec![
        (
            "If-Match on ACTIVE before the model has settled",
            vec![("if-match", "$ETAG"), ("match-for-version-type", "ACTIVE")],
            412,
        ),
        (
            "If-None-Match on ACTIVE before the model has settled",
            vec![("if-none-match", "*"), ("match-for-version-type", "ACTIVE")],
            202,
        ),
    ];

    for (case, headers, expected) in &rows {
        // A model created and left alone: the transition it owes is still due,
        // so this read reports the LATEST version's tag and no ACTIVE version
        // exists behind it.
        let model_id = create_model(&router, case, base_properties()).await;
        let tag = describe_e_tag(&router, &model_id).await;
        // The properties this update KEEPS, named by their stored ids.
        let kept = stored_properties(&router, &model_id).await;
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| {
                (*name, if *value == "$ETAG" { tag.as_str() } else { *value })
            })
            .collect();

        let (status, answered) = update_model(
            &router,
            &model_id,
            json!({"assetModelName": "Conditioned", "assetModelProperties": kept}),
            &headers,
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );

        let after = describe_model(&router, &model_id).await;
        if *expected == 412 {
            assert_eq!(
                error_code(&answered),
                "PreconditionFailedException",
                "case {case:?}: unexpected error code in {answered}"
            );
            assert_eq!(
                after["assetModelName"].as_str(),
                Some(*case),
                "case {case:?}: a refused update must leave the name alone, got {after}"
            );
            assert_eq!(
                describe_e_tag(&router, &model_id).await,
                tag,
                "case {case:?}: a refused update must leave the ETag alone"
            );
        } else {
            assert_eq!(
                after["assetModelName"].as_str(),
                Some("Conditioned"),
                "case {case:?}: the accepted update must have landed, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn every_asset_of_the_model_inherits_the_updated_definition() {
    let (router, engine) = router_with_engine();
    let model_id = create_model(&router, "Boiler", base_properties()).await;
    // Two assets, so the propagation is about the model's assets rather than one.
    let first = create_asset(&router, "Boiler1", &model_id).await;
    let second = create_asset(&router, "Boiler2", &model_id).await;
    // An asset of ANOTHER model, which this update must not touch.
    let other_model = create_model(&router, "Pump", base_properties()).await;
    let other_asset = create_asset(&router, "Pump1", &other_model).await;

    let described = describe_asset(&router, &first).await;
    let property_ids = ids_by_name(&described["assetProperties"]);
    let removed_id = id_of(&property_ids, "Temperature").expect("a Temperature property");
    let kept_id = id_of(&property_ids, "Serial").expect("a Serial property");

    // Give both properties of both assets stored data, so the deletion the removal
    // owes is observable.
    let seconds = crate::support::now_seconds() - 60;
    for asset_id in [&first, &second] {
        for (property, value) in [(&removed_id, json!({"doubleValue": 42.5})), (&kept_id, json!({"stringValue": "SN-1"}))] {
            let (status, body) = call(
                &router,
                "POST",
                BATCH_PUT,
                Some(json!({"entries": [{
                    "entryId": "seed",
                    "assetId": asset_id,
                    "propertyId": property,
                    "propertyValues": [{
                        "value": value,
                        "timestamp": {"timeInSeconds": seconds},
                    }],
                }]})),
            )
            .await;
            assert_eq!(status, 200, "seeding {property}: got {status} {body}");
            assert_eq!(
                body["errorEntries"].as_array().map_or(0, Vec::len),
                0,
                "seeding {property}: the write was refused: {body}"
            );
        }
    }

    // The update removes `Temperature` and adds `Pressure`, keeping `Serial`.
    let model = describe_model(&router, &model_id).await;
    let properties: Vec<Value> = model["assetModelProperties"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|property| property["name"] != "Temperature")
        .chain([json!({
            "name": "Pressure", "dataType": "INTEGER", "unit": "kPa",
            "type": {"measurement": {}},
        })])
        .collect();
    let (status, answered) = update_model(
        &router,
        &model_id,
        json!({"assetModelName": "Boiler", "assetModelProperties": properties}),
        &[],
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {answered}");

    for asset_id in [&first, &second] {
        let asset = describe_asset(&router, asset_id).await;
        assert_eq!(
            names(&asset["assetProperties"]),
            vec!["Serial".to_string(), "Pressure".to_string()],
            "asset {asset_id}: expected the updated definition, got {asset}"
        );
        let ids = ids_by_name(&asset["assetProperties"]);
        assert_eq!(
            id_of(&ids, "Serial").as_deref(),
            Some(kept_id.as_str()),
            "asset {asset_id}: the retained property must keep its id, got {asset}"
        );
        // The asset reports UPDATING while the transition this call owes settles.
        assert_eq!(
            asset["assetStatus"]["state"].as_str(),
            Some("UPDATING"),
            "asset {asset_id}: expected the UPDATING state, got {asset}"
        );
    }

    // "If you remove a property from an asset model, IoT SiteWise deletes all
    // previous data for that property" — and only for that property. Read through
    // the store, the one place the series live.
    let state = service_state(&*engine).expect("the iotsitewise store");
    let store = state.lock();
    for asset_id in [&first, &second] {
        let series = store.property_values.get(asset_id);
        assert!(
            series.is_none_or(|series| !series.contains_key(&removed_id)),
            "asset {asset_id}: the removed property's data must be deleted, got {series:?}"
        );
        assert!(
            store.latest_property_value(asset_id, &kept_id).is_some(),
            "asset {asset_id}: the retained property must keep its data"
        );
    }
    // The other model's asset is untouched: neither its definition nor its data.
    let untouched = store
        .assets
        .get(&other_asset)
        .expect("the other model's asset");
    assert_eq!(
        untouched
            .properties
            .iter()
            .map(|property| property.name.clone())
            .collect::<Vec<String>>(),
        vec!["Temperature".to_string(), "Serial".to_string()],
        "the other model's asset must keep its definition, got {untouched:?}"
    );
}

#[tokio::test]
async fn a_definition_past_a_published_quota_is_refused() {
    let router = router();

    // The published per-model quotas, from the input members' own documentation:
    // "You can specify up to 200 properties per asset model" and "You can specify
    // up to 10 hierarchies per asset model". Each bound is tested AT the limit and
    // one past it.
    const MAX_PROPERTIES: usize = 200;
    const MAX_HIERARCHIES: usize = 10;

    fn properties(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|at| {
                    json!({
                        "name": format!("P{at}"), "dataType": "DOUBLE",
                        "type": {"measurement": {}},
                    })
                })
                .collect(),
        )
    }

    let model_id = create_model(&router, "Boiler", json!([])).await;
    for (case, count, expected) in [
        ("at the property quota", MAX_PROPERTIES, 202u16),
        ("one property past the quota", MAX_PROPERTIES + 1, 410),
    ] {
        let (status, answered) = update_model(
            &router,
            &model_id,
            json!({"assetModelName": "Boiler", "assetModelProperties": properties(count)}),
            &[],
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );
        if expected == 410 {
            assert_eq!(
                error_code(&answered),
                "LimitExceededException",
                "case {case:?}: unexpected error code in {answered}"
            );
        }
    }

    // One child model per hierarchy slot, plus one for the row past the bound.
    let mut children = Vec::new();
    for at in 0..=MAX_HIERARCHIES {
        children.push(create_model(&router, &format!("Child{at}"), json!([])).await);
    }
    for (case, count, expected) in [
        ("at the hierarchy quota", MAX_HIERARCHIES, 202u16),
        ("one hierarchy past the quota", MAX_HIERARCHIES + 1, 410),
    ] {
        let hierarchies: Vec<Value> = children
            .iter()
            .take(count)
            .enumerate()
            .map(|(at, child)| json!({"name": format!("H{at}"), "childAssetModelId": child}))
            .collect();
        let (status, answered) = update_model(
            &router,
            &model_id,
            json!({"assetModelName": "Boiler", "assetModelHierarchies": hierarchies}),
            &[],
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );
        if expected == 410 {
            assert_eq!(
                error_code(&answered),
                "LimitExceededException",
                "case {case:?}: unexpected error code in {answered}"
            );
        }
    }
}

#[tokio::test]
async fn an_id_that_names_no_stored_asset_model_is_refused() {
    let router = router();
    let model_id = create_model(&router, "Boiler", base_properties()).await;

    // Rows differ along WHICH id of the request names nothing, and in which of the
    // two `CustomID` forms it is written.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "the model to update, by UUID",
            UNKNOWN_ID.to_string(),
            json!({"assetModelName": "Boiler"}),
        ),
        (
            "the model to update, by externalId label",
            "externalId:no.such.model".to_string(),
            json!({"assetModelName": "Boiler"}),
        ),
        (
            "a hierarchy's child model, by UUID",
            model_id.clone(),
            json!({
                "assetModelName": "Boiler",
                "assetModelHierarchies": [{"name": "Sensors", "childAssetModelId": UNKNOWN_ID}],
            }),
        ),
        (
            "a hierarchy's child model, by externalId label",
            model_id.clone(),
            json!({
                "assetModelName": "Boiler",
                "assetModelHierarchies": [
                    {"name": "Sensors", "childAssetModelId": "externalId:no.such.model"},
                ],
            }),
        ),
    ];

    for (case, target, body) in &rows {
        let (status, answered) = update_model(&router, target, body.clone(), &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {answered}"
        );
    }

    // Nothing of the refused requests landed.
    let after = describe_model(&router, &model_id).await;
    assert_eq!(
        names(&after["assetModelHierarchies"]),
        Vec::<String>::new(),
        "a refused update must leave the hierarchies alone, got {after}"
    );
    assert_eq!(
        names(&after["assetModelProperties"]),
        vec!["Temperature".to_string(), "Serial".to_string()],
        "a refused update must leave the properties alone, got {after}"
    );
}
