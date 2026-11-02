// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the loop a client actually walks: take a property id from
//! the asset's PUBLISHED views and ask the data plane for that property's history.
//!
//! The rule this suite pins: every property id this service publishes as a property
//! of an asset — `ListAssetProperties` with `filter=ALL`, which enumerates the
//! asset's own properties together with the ones its model's composite models
//! contribute, and `DescribeAssetProperty`, which answers 200 for each of them — is
//! addressable by `GetAssetPropertyValueHistory` through the same
//! `(assetId, propertyId)` pair, which answers that property's samples inside the
//! requested window, and an EMPTY history when the property holds none. A published
//! property with no samples is an empty 200, never a `ResourceNotFoundException`:
//! only a pair naming no published property of the asset is the modelled refusal.
//!
//! The table's rows differ along WHERE the property is declared (on the asset model
//! itself, on a composite model declared with the model, on one added afterwards, on
//! one nested inside another, and on the component model a component-model-based
//! composite model reuses) and along its `PropertyDataType`; the asset's own
//! property is the control row. Every row is driven from the id the LISTING reported
//! rather than from a hand-written id, so a property the service publishes and this
//! read cannot resolve fails here.
//!
//! The read itself is asked in several forms — no window, both orderings, an
//! explicit window, a one-sample page, a quality filter and a window that excludes
//! the sample — so the resolution is pinned independently of which samples the query
//! selects.
//!
//! The wire harness (`router`, `call`, `create_asset`, …) lives in the
//! `asset_lifecycle` suite, the composite-model create in
//! `asset_model_composite_model` and the clock anchoring in `support`; this file
//! reuses all three rather than keeping a copy.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, error_code, is_modelled_id, router};
use super::asset_model_composite_model::create_composite;

/// Where `BatchPutAssetPropertyValue` is bound: `POST /properties`.
const BATCH_PUT: &str = "/properties";

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// An epoch second as the ISO-8601 `date-time` an `@httpQuery`-bound timestamp
/// travels as: `startDate` and `endDate` carry no `@timestampFormat`, so they use
/// Smithy's default format for a query-bound timestamp.
fn iso(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// The instant every property's one sample is written at.
///
/// `BatchPutAssetPropertyValue` ingests only TQVs inside
/// `[now - 7 days, now + 10 minutes]`, so the second comes from the shared anchor,
/// which is fixed once per test binary and sits inside that window.
fn sample_second() -> i64 {
    super::support::anchor()
}

/// One property the fixture asset carries: what the row varies, the property's
/// name, and the `PropertyDataType` it is declared with.
struct Declared {
    /// What the row varies: the case name every assertion carries.
    case: &'static str,
    /// The property's name, which is how the row finds the id the listing
    /// published for it.
    name: &'static str,
    /// The declared data type, which decides which `Variant` member a written
    /// sample carries.
    data_type: &'static str,
}

/// Every property the fixture asset carries, one row per place an asset carries a
/// property and per measurement data type.
///
/// The data types are the measurement ones of `PropertyDataType`. Source:
/// `smithy_query(query="shape", shape="PropertyDataType")` — STRING | INTEGER |
/// DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON, of which the last four
/// are not measurement types.
const CATALOG: [Declared; 9] = [
    Declared {
        case: "the asset's own DOUBLE measurement",
        name: "Temperature",
        data_type: "DOUBLE",
    },
    Declared {
        case: "the asset's own INTEGER measurement",
        name: "Cycles",
        data_type: "INTEGER",
    },
    Declared {
        case: "the asset's own STRING measurement",
        name: "Status",
        data_type: "STRING",
    },
    Declared {
        case: "the asset's own BOOLEAN measurement",
        name: "Running",
        data_type: "BOOLEAN",
    },
    Declared {
        case: "a property of an AWS/ALARM composite model declared with the asset model",
        name: "AlarmValue",
        data_type: "DOUBLE",
    },
    Declared {
        case: "a property of a CUSTOM composite model added after the asset model",
        name: "Throughput",
        data_type: "DOUBLE",
    },
    Declared {
        case: "a STRING property of a CUSTOM composite model",
        name: "Grade",
        data_type: "STRING",
    },
    Declared {
        case: "a property of a composite model nested inside another",
        name: "Deep",
        data_type: "INTEGER",
    },
    Declared {
        case: "a property a component model contributes through the composite model that reuses it",
        name: "Measured",
        data_type: "DOUBLE",
    },
];

/// The names of the properties the asset model declares DIRECTLY — the ones
/// `ListAssetProperties` reports under its default `BASE` filter.
const BASE_NAMES: [&str; 4] = ["Temperature", "Cycles", "Status", "Running"];

/// A distinct `Variant` per row, so a read that served one row's series for
/// another would be caught rather than agreeing with itself.
fn variant(data_type: &str, at: usize) -> Value {
    match data_type {
        "INTEGER" => json!({"integerValue": 100 + at as i64}),
        "STRING" => json!({"stringValue": format!("reading-{at}")}),
        "BOOLEAN" => json!({"booleanValue": at.is_multiple_of(2)}),
        _ => json!({"doubleValue": 10.5 + at as f64}),
    }
}

/// One published property under test: the id the listing reported, the row it
/// belongs to, and the sample written to it.
struct Published {
    /// The row's case name.
    case: &'static str,
    /// The property's name, as the listing's `path` reports it.
    name: String,
    /// The `propertyId` the listing published.
    property_id: String,
    /// The `assetCompositeModelId` the listing reported, when the property is a
    /// composite model's.
    composite_model_id: Option<String>,
    /// The one sample this property is written.
    sample: Value,
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

/// An asset carrying one property in every place an asset carries one, answered as
/// its id.
///
/// `label` keeps the names of two fixtures in one test binary apart.
async fn fixture(router: &MultiServiceRouter, label: &str) -> String {
    // A COMPONENT_MODEL with a property of its own: a component-model-based
    // composite model stores no properties, and the component model it names is
    // what provides them.
    let (status, component) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{label}Thermostat"),
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Measured", "dataType": "DOUBLE", "unit": "Celsius",
                 "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "create component model: got {status} {component}");
    let component_id = component["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create component model: no assetModelId in {component}"))
        .to_string();

    // The asset model: its own properties and one composite model declared with it.
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{label}Boiler"),
            "assetModelProperties": [
                {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius",
                 "type": {"measurement": {}}},
                {"name": "Cycles", "dataType": "INTEGER", "unit": "Count",
                 "type": {"measurement": {}}},
                {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
                {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
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

    // Three composite models added after the model was created: a custom one, one
    // nested inside it, and one that reuses the component model above.
    let shift = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Shift",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Throughput", "dataType": "DOUBLE", "unit": "units/h",
                 "type": {"measurement": {}}},
                {"name": "Grade", "dataType": "STRING", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "NestedShift",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": shift,
            "assetModelCompositeModelProperties": [
                {"name": "Deep", "dataType": "INTEGER", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": component_id,
        }),
    )
    .await;

    create_asset(router, &format!("{label}Boiler-1"), &model_id).await
}

/// Every summary `ListAssetProperties` publishes for `asset_id` under `filter`,
/// walked to the end of the enumeration through `nextToken`.
async fn published_summaries(
    router: &MultiServiceRouter,
    asset_id: &str,
    filter: &str,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = format!("filter={filter}");
        if let Some(token) = &token {
            query.push_str(&format!("&nextToken={token}"));
        }
        let (status, body) = call(
            router,
            "GET",
            &format!("/assets/{asset_id}/properties?{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "ListAssetProperties filter={filter}: got {status} {body}"
        );
        summaries.extend(
            body["assetPropertySummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("no assetPropertySummaries in {body}"))
                .iter()
                .cloned(),
        );
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    summaries
}

/// A summary's property name: the LAST segment of the `path`, which is "the
/// structured path to the property from the root of the asset" and so ends at the
/// property itself.
fn summary_name(summary: &Value) -> String {
    summary["path"]
        .as_array()
        .and_then(|path| path.last())
        .and_then(|segment| segment["name"].as_str())
        .unwrap_or_else(|| panic!("no path naming the property in {summary}"))
        .to_string()
}

/// The published properties of the fixture asset, one per [`CATALOG`] row, built
/// from what `ListAssetProperties` reported rather than from any assumed id.
async fn published(router: &MultiServiceRouter, asset_id: &str) -> Vec<Published> {
    let summaries = published_summaries(router, asset_id, "ALL").await;

    // The fixture is what this suite thinks it is: the published set is exactly
    // the catalog, so no row below is silently skipped and nothing unexpected is
    // published either.
    let mut listed: Vec<String> = summaries.iter().map(summary_name).collect();
    listed.sort();
    let mut expected: Vec<String> = CATALOG.iter().map(|row| row.name.to_string()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "ListAssetProperties filter=ALL must publish every property the asset carries, got \
         {summaries:?}"
    );

    CATALOG
        .iter()
        .enumerate()
        .map(|(at, row)| {
            let summary = summaries
                .iter()
                .find(|summary| summary_name(summary) == row.name)
                .unwrap_or_else(|| {
                    panic!("case {:?}: no summary named {:?}", row.case, row.name)
                });
            let property_id = summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: no id in {summary}", row.case))
                .to_string();
            assert!(
                is_modelled_id(&property_id),
                "case {:?}: expected a modelled property id, got {property_id:?}",
                row.case
            );
            Published {
                case: row.case,
                name: row.name.to_string(),
                property_id,
                composite_model_id: summary["assetCompositeModelId"]
                    .as_str()
                    .map(ToString::to_string),
                sample: variant(row.data_type, at),
            }
        })
        .collect()
}

/// One shape of the history read every published property is asked in.
struct ReadForm {
    /// What the row varies: the case name every assertion carries.
    case: &'static str,
    /// The query members appended after `assetId` and `propertyId`.
    query: String,
    /// Whether this form's window and filter select the one written sample.
    covers_sample: bool,
}

/// Every form the read is asked in: no window, both orderings, an explicit window
/// around the sample, a one-sample page, the quality the sample carries, and a
/// window that lies entirely after the sample.
fn read_forms() -> Vec<ReadForm> {
    let sample = sample_second();
    vec![
        ReadForm {
            case: "no window and the default ordering",
            query: String::new(),
            covers_sample: true,
        },
        ReadForm {
            case: "no window, newest first",
            query: "&timeOrdering=DESCENDING".to_string(),
            covers_sample: true,
        },
        ReadForm {
            case: "no window, oldest first",
            query: "&timeOrdering=ASCENDING".to_string(),
            covers_sample: true,
        },
        ReadForm {
            case: "a window around the sample",
            query: format!(
                "&startDate={}&endDate={}",
                iso(sample - 60),
                iso(sample + 60)
            ),
            covers_sample: true,
        },
        ReadForm {
            case: "a one-sample page",
            query: "&maxResults=1".to_string(),
            covers_sample: true,
        },
        ReadForm {
            case: "the quality the sample carries",
            query: "&qualities=GOOD".to_string(),
            covers_sample: true,
        },
        ReadForm {
            case: "a window entirely after the sample",
            query: format!(
                "&startDate={}&endDate={}",
                iso(sample + 60),
                iso(sample + 120)
            ),
            covers_sample: false,
        },
    ]
}

/// `GET /properties/history` for one pair, in one read form.
async fn history(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    form: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/properties/history?assetId={asset_id}&propertyId={property_id}{form}"),
        None,
    )
    .await
}

/// The samples a history answer reports.
fn reported(case: &str, body: &Value) -> Vec<Value> {
    body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {body}"))
        .clone()
}

#[tokio::test]
async fn every_published_property_id_is_answered_by_the_history_read() {
    let router = router();
    let asset_id = fixture(&router, "Published").await;
    let properties = published(&router, &asset_id).await;

    // The default `BASE` filter is the narrower view of the same enumeration: the
    // asset's own properties only. Both views publish ids this read must resolve.
    let mut base: Vec<String> = published_summaries(&router, &asset_id, "BASE")
        .await
        .iter()
        .map(summary_name)
        .collect();
    base.sort();
    let mut expected_base: Vec<String> = BASE_NAMES.iter().map(ToString::to_string).collect();
    expected_base.sort();
    assert_eq!(
        base, expected_base,
        "ListAssetProperties filter=BASE must publish the asset's own properties only, got \
         {base:?}"
    );

    let forms = read_forms();

    for property in &properties {
        let case = property.case;
        let name = &property.name;
        let id = &property.property_id;

        // What publishes the pair: `DescribeAssetProperty` answers 200 for it,
        // reporting a composite model's property under `compositeModel` and one of
        // the asset's own under `assetProperty`.
        let (status, described) =
            call(&router, "GET", &format!("/assets/{asset_id}/properties/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty for {name:?} got {status} {described}"
        );
        let reported_id = match &property.composite_model_id {
            Some(_) => described["compositeModel"]["assetProperty"]["id"].as_str(),
            None => described["assetProperty"]["id"].as_str(),
        };
        assert_eq!(
            reported_id,
            Some(id.as_str()),
            "case {case:?}: DescribeAssetProperty must report the published id, got {described}"
        );

        // A published property that holds no samples is an EMPTY history, never a
        // refusal: the read resolves the property the describe published.
        for form in &forms {
            let shape = form.case;
            let (status, body) = history(&router, &asset_id, id, &form.query).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {shape:?}: expected the empty history of a published property, \
                 got {status} {body}"
            );
            let samples = reported(case, &body);
            assert!(
                samples.is_empty(),
                "case {case:?} / {shape:?}: expected no samples before any write, got {samples:?}"
            );
            assert!(
                body["nextToken"].is_null(),
                "case {case:?} / {shape:?}: an empty history has no next page, got {body}"
            );
        }
    }

    // One sample per published property, written through the ingest path by the
    // same pair the listing published.
    let entries: Vec<Value> = properties
        .iter()
        .enumerate()
        .map(|(at, property)| {
            json!({
                "entryId": format!("e{at}"),
                "assetId": asset_id,
                "propertyId": property.property_id,
                "propertyValues": [{
                    "value": property.sample,
                    "timestamp": {"timeInSeconds": sample_second(), "offsetInNanos": 0},
                    "quality": "GOOD",
                }],
            })
        })
        .collect();
    let (status, body) = call(
        &router,
        "POST",
        BATCH_PUT,
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(status, 200, "the write got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "every sample of a published property of this asset must be accepted, got {body}"
    );

    for property in &properties {
        let case = property.case;
        for form in &forms {
            let shape = form.case;
            let (status, body) = history(&router, &asset_id, &property.property_id, &form.query)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {shape:?}: GetAssetPropertyValueHistory got {status} {body}"
            );
            let samples = reported(case, &body);
            let expected = usize::from(form.covers_sample);
            assert_eq!(
                samples.len(),
                expected,
                "case {case:?} / {shape:?}: expected {expected} sample(s), got {samples:?}"
            );
            if let Some(sample) = samples.first() {
                assert_eq!(
                    sample["value"], property.sample,
                    "case {case:?} / {shape:?}: expected the value written to this property, got \
                     {sample}"
                );
                assert_eq!(
                    sample["timestamp"]["timeInSeconds"].as_i64(),
                    Some(sample_second()),
                    "case {case:?} / {shape:?}: expected the instant written, got {sample}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_composite_model_added_after_the_asset_is_published_and_answered_too() {
    let router = router();
    let asset_id = fixture(&router, "Late").await;
    // The fixture's own published set, so the row below is about what the new
    // composite model ADDS to it.
    let before: Vec<String> = published_summaries(&router, &asset_id, "ALL")
        .await
        .iter()
        .map(summary_name)
        .collect();

    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let model_id = asset["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeAsset reported no assetModelId: {asset}"))
        .to_string();

    // "A composite model added to the model reaches the assets already created
    // from it": the property it contributes becomes a property of this asset.
    composite_id(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Late",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "LateValue", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;

    let summaries = published_summaries(&router, &asset_id, "ALL").await;
    let case = "a property of a composite model added after the asset was created";
    let summary = summaries
        .iter()
        .find(|summary| summary_name(summary) == "LateValue")
        .unwrap_or_else(|| {
            panic!("case {case:?}: the new property must be published, got {summaries:?}")
        });
    let property_id = summary["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {summary}"))
        .to_string();
    let still_published: Vec<String> = summaries.iter().map(summary_name).collect();
    assert!(
        before.iter().all(|name| still_published.contains(name)),
        "case {case:?}: the properties published before must stay published, got \
         {still_published:?}"
    );

    // Before any write: the empty history of a property the service publishes.
    let (status, body) = history(&router, &asset_id, &property_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the empty history of a published property, got {status} {body}"
    );
    assert!(
        reported(case, &body).is_empty(),
        "case {case:?}: expected no samples before any write, got {body}"
    );

    // And the samples written to it afterwards.
    let sample = json!({"doubleValue": 42.5});
    let (status, body) = call(
        &router,
        "POST",
        BATCH_PUT,
        Some(json!({"entries": [{
            "entryId": "late",
            "assetId": asset_id,
            "propertyId": property_id,
            "propertyValues": [{
                "value": sample,
                "timestamp": {"timeInSeconds": sample_second(), "offsetInNanos": 0},
                "quality": "GOOD",
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: the write got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: the sample must be accepted, got {body}"
    );

    let (status, body) = history(&router, &asset_id, &property_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAssetPropertyValueHistory got {status} {body}"
    );
    let samples = reported(case, &body);
    assert_eq!(
        samples.len(),
        1,
        "case {case:?}: expected the one written sample, got {samples:?}"
    );
    assert_eq!(
        samples[0]["value"], sample,
        "case {case:?}: expected the value written, got {}",
        samples[0]
    );
}

#[tokio::test]
async fn a_pair_naming_no_published_property_is_the_modelled_refusal() {
    let router = router();
    let asset_id = fixture(&router, "Refused").await;
    let properties = published(&router, &asset_id).await;

    // A second asset of its own model: a property id that is somebody else's is not
    // a published property of this asset, however well shaped it is.
    let other_asset = fixture(&router, "Other").await;
    let other_properties = published(&router, &other_asset).await;
    let other_composite = other_properties
        .iter()
        .find(|property| property.composite_model_id.is_some())
        .map(|property| property.property_id.clone())
        .expect("a composite model's property on the other asset");
    let own_composite = properties
        .iter()
        .find(|property| property.composite_model_id.is_some())
        .map(|property| property.property_id.clone())
        .expect("a composite model's property on this asset");

    let cases = [
        (
            "an id no create minted",
            asset_id.clone(),
            ABSENT_ID.to_string(),
        ),
        (
            "another asset's composite model property",
            asset_id.clone(),
            other_composite,
        ),
        (
            "this asset's composite model property against the other asset",
            other_asset.clone(),
            own_composite,
        ),
        (
            "the asset's own id used as a property id",
            asset_id.clone(),
            asset_id.clone(),
        ),
    ];

    for (case, asset, property) in cases {
        let (status, body) = history(&router, &asset, &property, "").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The asset {asset} has no property {property}.").as_str()),
            "case {case:?}: unexpected message in {body}"
        );

        // The pair the history read refuses is the pair the published views refuse
        // too: the two agree on what a property of this asset is.
        let (status, described) = call(
            &router,
            "GET",
            &format!("/assets/{asset}/properties/{property}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: DescribeAssetProperty expected 404, got {status} {described}"
        );
    }
}
