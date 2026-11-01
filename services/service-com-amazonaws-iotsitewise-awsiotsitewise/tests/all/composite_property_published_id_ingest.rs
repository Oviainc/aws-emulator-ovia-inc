// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the INGEST path of a property an asset carries through one
//! of its asset model's composite models, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule this suite pins: an asset carries ONE flat set of properties — those
//! its asset model declares directly together with those the model's composite
//! models contribute, whenever a composite model was added — and every operation
//! that names a property by `(assetId, propertyId)` reaches the same property, so a
//! `propertyId` the asset's own property listing publishes is one the write path
//! resolves, as is the `propertyAlias` bound to that property.
//!
//! Every id a row addresses is taken from `ListAssetProperties` with `filter=ALL`,
//! the asset-level listing a client reads a property id from, and cross-checked
//! against `DescribeAssetProperty` — so the suite starts from the premise the rule
//! rests on (these reads PUBLISH the property as the asset's) and then writes the
//! id they published.
//!
//! The rows differ along WHERE the property is declared (on the model itself, on a
//! composite model declared with the model, on one added AFTER the asset was
//! created, on one nested inside such a one, and on the component model a
//! component-model-based composite model lends it through) and along its
//! `PropertyDataType`. The asset's own top-level property is the control row: every
//! row must be served identically.
//!
//! `composite_model_property_streams` pins the same reach for an asset whose
//! composite models all predate it, addressed by ids read off the MODEL; the
//! composite models here are created after the asset exists, so each row's property
//! is materialised onto a STORED asset rather than at its create.
//!
//! The wire harness (`router`, `call`, `create_asset`, …) lives in the
//! `asset_lifecycle` suite, the composite-model create in
//! `asset_model_composite_model` and the clock anchoring in `support`; this file
//! reuses all three rather than keeping a copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, router};
use super::asset_model_composite_model::create_composite;

/// Where `BatchPutAssetPropertyValue` is bound: `POST /properties`.
const BATCH_PUT: &str = "/properties";

/// One row of the table: a property of the asset under test, and where the asset
/// carries it from.
struct Row {
    /// What the row varies: the case name every assertion carries.
    case: &'static str,
    /// The property's name, which is how the row finds it in the listing — the
    /// `propertyId` itself is never assumed, always read back.
    name: &'static str,
    /// The property's declared `PropertyDataType`, which decides which `Variant`
    /// member a sample written to it carries. Catalog source:
    /// `smithy_query(query="shape", shape="PropertyDataType")` — STRING | INTEGER |
    /// DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON, of which the first
    /// four are the ones a measurement can carry.
    data_type: &'static str,
    /// Whether a composite model of the asset's model contributes it, which decides
    /// whether the listing reports an `assetCompositeModelId` beside it and which
    /// member `DescribeAssetProperty` reports it under.
    contributed: bool,
}

/// The properties of the fixture asset, one row each: the control row first, then
/// one per place a composite model can contribute a property from, spanning the
/// four `PropertyDataType`s a measurement carries.
const ROWS: [Row; 6] = [
    Row {
        case: "the asset's own top-level property",
        name: "Temperature",
        data_type: "DOUBLE",
        contributed: false,
    },
    Row {
        case: "a property of a composite model declared with the asset model",
        name: "AlarmValue",
        data_type: "DOUBLE",
        contributed: true,
    },
    Row {
        case: "an INTEGER property of a composite model added after the asset was created",
        name: "Cycles",
        data_type: "INTEGER",
        contributed: true,
    },
    Row {
        case: "a BOOLEAN property of a composite model added after the asset was created",
        name: "Running",
        data_type: "BOOLEAN",
        contributed: true,
    },
    Row {
        case: "a STRING property of a composite model nested inside another",
        name: "Deep",
        data_type: "STRING",
        contributed: true,
    },
    Row {
        case: "a property a component model lends through the composite model reusing it",
        name: "Lent",
        data_type: "STRING",
        contributed: true,
    },
];

/// The `Variant` member a property of `data_type` holds its reading in.
fn value_member(data_type: &str) -> &'static str {
    match data_type {
        "INTEGER" => "integerValue",
        "STRING" => "stringValue",
        "BOOLEAN" => "booleanValue",
        _ => "doubleValue",
    }
}

/// The `Variant` carrying the reading `seed` stands for, in the member a property
/// of `data_type` holds its readings in — so one table drives every data type and
/// two different seeds are two distinguishable readings of every one of them.
fn variant(data_type: &str, seed: i64) -> Value {
    match data_type {
        "INTEGER" => json!({"integerValue": seed}),
        "STRING" => json!({"stringValue": format!("reading-{seed}")}),
        "BOOLEAN" => json!({"booleanValue": seed % 2 == 1}),
        _ => json!({"doubleValue": seed as f64 + 0.5}),
    }
}

/// The seconds, after the shared anchor, of the samples a row writes: two through
/// the `(assetId, propertyId)` pair, then one more through the alias bound to the
/// same property.
const SAMPLE_OFFSETS: [i64; 3] = [10, 20, 30];

/// One TQV at `offset` seconds past the anchor, carrying the reading `seed` stands
/// for.
fn sample(data_type: &str, offset: i64, seed: i64) -> Value {
    json!({
        "value": variant(data_type, seed),
        "timestamp": {"timeInSeconds": super::support::anchor() + offset},
        "quality": "GOOD",
    })
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

/// An asset of a model that gains composite models AFTER the asset exists, so the
/// properties the rows address are materialised onto a stored asset.
///
/// `label` keeps the names of two fixtures in one test binary apart.
async fn boiler(router: &MultiServiceRouter, label: &str) -> String {
    // A COMPONENT_MODEL with a property of its own, reused below: a
    // component-model-based composite model stores no properties, and the component
    // model it names is what lends them.
    let (status, component) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{label}Thermostat"),
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Lent", "dataType": "STRING", "type": {"measurement": {}}},
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
                    {"name": "AlarmValue", "dataType": "DOUBLE", "type": {"measurement": {}}},
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

    // The asset is created BEFORE the composite models below, so what it carries
    // from them is materialised onto the stored asset rather than at its create.
    let asset_id = create_asset(router, &format!("{label}Boiler-1"), &model_id).await;

    let shift = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Shift",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Cycles", "dataType": "INTEGER", "type": {"measurement": {}}},
                {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    let _nested = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "NestedShift",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": shift,
            "assetModelCompositeModelProperties": [
                {"name": "Deep", "dataType": "STRING", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    let _reused = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": component_id,
        }),
    )
    .await;

    asset_id
}

/// The `(propertyId, assetCompositeModelId)` `ListAssetProperties` with `filter=ALL`
/// publishes for the property named `name` of `asset_id`.
///
/// A summary carries no `name` of its own: its `path` is "the structured path to the
/// property from the root of the asset", whose LAST segment is the property itself,
/// so that segment's name is what names a summary.
async fn published(
    router: &MultiServiceRouter,
    asset_id: &str,
    case: &str,
    name: &str,
) -> (String, Option<String>) {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAssetProperties got {status} {body}"
    );
    let summaries = body["assetPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertySummaries in {body}"))
        .clone();
    let summary = summaries
        .iter()
        .find(|summary| {
            summary["path"]
                .as_array()
                .and_then(|path| path.last())
                .is_some_and(|segment| segment["name"] == *name)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must publish the property {name}, got {body}")
        });
    let id = summary["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id on the summary {summary}"))
        .to_string();
    let composite = summary["assetCompositeModelId"]
        .as_str()
        .map(ToString::to_string);
    (id, composite)
}

/// `BatchPutAssetPropertyValue` with one entry carrying `values`, which must be
/// accepted whole: a 200 whose `errorEntries` is empty.
async fn put_accepted(router: &MultiServiceRouter, case: &str, entry: Value) {
    let (status, body) = call(router, "POST", BATCH_PUT, Some(json!({"entries": [entry]}))).await;
    assert_eq!(status, 200, "case {case:?}: the write got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: every sample of a property of this asset must be ingested, got {body}"
    );
}

/// The latest sample `GetAssetPropertyValue` reports for `(asset_id, property_id)`,
/// as the single `Variant` member it must carry.
async fn latest_reading(
    router: &MultiServiceRouter,
    case: &str,
    asset_id: &str,
    property_id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={property_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAssetPropertyValue got {status} {body}"
    );
    let reported = body["propertyValue"]["value"].clone();
    assert_eq!(
        reported.as_object().map(serde_json::Map::len),
        Some(1),
        "case {case:?}: a Variant carries a value of a single type only, got {body}"
    );
    reported
}

#[tokio::test]
async fn a_property_id_the_asset_listing_publishes_is_one_the_ingest_path_resolves() {
    let router = router();
    let asset_id = boiler(&router, "Published").await;

    for (at, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let seed = 100 * at as i64;
        let (property_id, composite) = published(&router, &asset_id, case, row.name).await;

        // The premise the rule rests on: the listing reports the property AS a
        // property of this asset, naming the composite model that contributes it
        // when one does and naming none for the asset's own.
        assert_eq!(
            composite.is_some(),
            row.contributed,
            "case {case:?}: expected assetCompositeModelId {}, got {composite:?}",
            if row.contributed { "set" } else { "absent" }
        );

        // And `DescribeAssetProperty` answers the same pair, reporting a
        // contributed property under `compositeModel.assetProperty` and one of the
        // asset's own under `assetProperty`.
        let (status, described) = call(
            &router,
            "GET",
            &format!("/assets/{asset_id}/properties/{property_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {described}"
        );
        let reported = if row.contributed {
            described["compositeModel"]["assetProperty"]["id"].clone()
        } else {
            described["assetProperty"]["id"].clone()
        };
        assert_eq!(
            reported.as_str(),
            Some(property_id.as_str()),
            "case {case:?}: DescribeAssetProperty must report the published id, got {described}"
        );

        // The write the published id must be usable for: both samples ingested.
        put_accepted(
            &router,
            case,
            json!({
                "entryId": format!("pair-{at}"),
                "assetId": asset_id,
                "propertyId": property_id,
                "propertyValues": [
                    sample(row.data_type, SAMPLE_OFFSETS[0], seed),
                    sample(row.data_type, SAMPLE_OFFSETS[1], seed + 1),
                ],
            }),
        )
        .await;

        // Read back: the newest sample is the latest value, and the series holds
        // exactly the two samples written, oldest first.
        let member = value_member(row.data_type);
        let newest = variant(row.data_type, seed + 1);
        let reading = latest_reading(&router, case, &asset_id, &property_id).await;
        assert_eq!(
            reading[member], newest[member],
            "case {case:?}: expected the latest {member} {}, got {reading}",
            newest[member]
        );

        let (status, history) = call(
            &router,
            "GET",
            &format!("/properties/history?assetId={asset_id}&propertyId={property_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAssetPropertyValueHistory got {status} {history}"
        );
        let samples: Vec<Value> = history["assetPropertyValueHistory"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {history}"))
            .iter()
            .map(|sample| sample["value"][member].clone())
            .collect();
        let expected: Vec<Value> = [seed, seed + 1]
            .into_iter()
            .map(|seed| variant(row.data_type, seed)[member].clone())
            .collect();
        assert_eq!(
            samples, expected,
            "case {case:?}: expected the series {expected:?}, got {history}"
        );
    }
}

#[tokio::test]
async fn an_alias_bound_to_a_published_property_ingests_onto_that_propertys_series() {
    let router = router();
    let asset_id = boiler(&router, "Aliased").await;

    for (at, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let seed = 500 * at as i64;
        let (property_id, _) = published(&router, &asset_id, case, row.name).await;
        let alias = format!("/aliased/{}/{}", asset_id, row.name);

        // `UpdateAssetProperty` is the documented way to give an asset property a
        // data stream alias, whichever place the asset carries the property from.
        let (status, body) = call(
            &router,
            "PUT",
            &format!("/assets/{asset_id}/properties/{property_id}"),
            Some(json!({"propertyAlias": alias})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAssetProperty got {status} {body}"
        );

        // A write addressed by the alias and a write addressed by the pair land on
        // the SAME series, so the alias-addressed sample is what the pair reports.
        put_accepted(
            &router,
            case,
            json!({
                "entryId": format!("pair-{at}"),
                "assetId": asset_id,
                "propertyId": property_id,
                "propertyValues": [sample(row.data_type, SAMPLE_OFFSETS[0], seed)],
            }),
        )
        .await;
        put_accepted(
            &router,
            case,
            json!({
                "entryId": format!("alias-{at}"),
                "propertyAlias": alias,
                "propertyValues": [sample(row.data_type, SAMPLE_OFFSETS[2], seed + 1)],
            }),
        )
        .await;

        let member = value_member(row.data_type);
        let newest = variant(row.data_type, seed + 1);
        let reading = latest_reading(&router, case, &asset_id, &property_id).await;
        assert_eq!(
            reading[member], newest[member],
            "case {case:?}: the alias-addressed sample must be the latest value of the pair, \
             expected {member} {}, got {reading}",
            newest[member]
        );

        // And it APPENDED to that series rather than replacing it: the pair's
        // history holds both samples, the one each form addressed the property by.
        let (status, history) = call(
            &router,
            "GET",
            &format!("/properties/history?assetId={asset_id}&propertyId={property_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAssetPropertyValueHistory got {status} {history}"
        );
        let samples: Vec<Value> = history["assetPropertyValueHistory"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {history}"))
            .iter()
            .map(|sample| sample["value"][member].clone())
            .collect();
        let expected: Vec<Value> = [seed, seed + 1]
            .into_iter()
            .map(|seed| variant(row.data_type, seed)[member].clone())
            .collect();
        assert_eq!(
            samples, expected,
            "case {case:?}: the pair-addressed and the alias-addressed sample must both be in \
             the one series, expected {expected:?}, got {history}"
        );
    }
}
