// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetAssetPropertyAggregates` addressed by
//! `(assetId, propertyId)`, driven over HTTP through the same `MultiServiceRouter`
//! the emulator boots with.
//!
//! The rule these suites pin: a property an asset carries is addressed by the
//! `(assetId, propertyId)` pair whichever place the asset carries it in — one of
//! its model's own top-level definitions, or one a composite model of that model
//! contributes, whenever that composite model was added — so the aggregate read
//! answers the windows of EVERY property the service itself reports as a property
//! of the asset (`ListAssetProperties`, `DescribeAssetProperty`), and a pair that
//! names no property of it is the modelled refusal.
//!
//! Each suite is a TABLE whose rows differ along WHERE the property is declared
//! (on the model itself, on a composite model declared with it, on one added after
//! the asset already existed, on one nested inside such a composite model, and on
//! the component model a component-model-based composite model reuses) and along
//! its data type, with the asset's own property as the control row. Every row is
//! then read at every modelled `resolution`, and in both time orderings at every
//! page size, and must be served identically.
//!
//! The wire harness (`router`, `call`, `create_asset`, …) lives in the
//! `asset_lifecycle` suite, the composite-model create in
//! `asset_model_composite_model` and the clock anchoring in `support`; this file
//! reuses all three rather than keeping a copy.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, error_code, property_id, router};
use super::asset_model_composite_model::create_composite;

/// Where `BatchPutAssetPropertyValue` is bound: `POST /properties`.
const BATCH_PUT: &str = "/properties";

/// Seconds in a day: the length of the longest modelled window, and the range
/// every row reads, so one range covers every window each resolution cuts the
/// row's readings into.
const DAY: i64 = 86_400;

/// Every `Resolution` an aggregate read may name, with the length in seconds of
/// the window it names.
///
/// Source: `smithy_query(query="shape", shape="Resolution")` — length 2..3 and
/// pattern `^1m|15m|1h|1d$`, i.e. one minute, fifteen minutes, one hour and one
/// day. Each length divides [`DAY`], which is what lets one midnight-aligned
/// instant start a window of all four.
const RESOLUTIONS: [(&str, i64); 4] = [("1m", 60), ("15m", 900), ("1h", 3_600), ("1d", DAY)];

/// The `aggregateTypes` every read in this file asks for, paired with the
/// `Aggregates` member each one reports under.
///
/// Source: `smithy_query(query="shape", shape="AggregateType")` — AVERAGE |
/// COUNT | MAXIMUM | MINIMUM | SUM | STANDARD_DEVIATION.
const AGGREGATES: [(&str, &str); 5] = [
    ("COUNT", "count"),
    ("SUM", "sum"),
    ("MINIMUM", "minimum"),
    ("MAXIMUM", "maximum"),
    ("AVERAGE", "average"),
];

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// How close two readings must be to count as the same number.
const EPSILON: f64 = 1e-9;

/// The seconds, after [`grid_start`], of the samples every row writes: three
/// instants one minute apart, so the shortest modelled window holds one each and
/// every longer one holds all three.
const SAMPLE_OFFSETS: [i64; 3] = [10, 70, 130];

/// The instant every row's readings are placed after: the UTC midnight at or
/// before the shared anchor.
///
/// The service buckets a series on the epoch-aligned resolution grid and every
/// modelled window length divides a day, so a midnight starts a window of each of
/// them. Taken from the shared anchor, so the readings stay inside the
/// `[now - 7 days, now + 10 minutes]` range `BatchPutAssetPropertyValue` ingests,
/// and fixed once per test binary.
fn grid_start() -> i64 {
    let anchor = super::support::anchor();
    anchor - anchor.rem_euclid(DAY)
}

/// An epoch second as the ISO-8601 `date-time` an `@httpQuery`-bound timestamp
/// travels as.
fn iso(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// One row of the table: a property of the asset under test, and where it is
/// declared.
struct Row {
    /// What the row varies: the case name every assertion carries.
    case: &'static str,
    /// The stored `propertyId` the row addresses.
    property_id: String,
    /// The property's declared `PropertyDataType`, which decides which `Variant`
    /// member a written sample carries.
    data_type: &'static str,
    /// The readings this row writes, one per [`SAMPLE_OFFSETS`] entry, oldest
    /// first.
    readings: [f64; 3],
}

/// The `Variant` carrying `reading` for a property of `data_type`.
fn variant(data_type: &str, reading: f64) -> Value {
    match data_type {
        "INTEGER" => json!({"integerValue": reading as i64}),
        _ => json!({"doubleValue": reading}),
    }
}

/// Assert `actual` is within [`EPSILON`] of `expected`.
fn assert_close(case: &str, what: &str, actual: Option<f64>, expected: f64) {
    assert!(
        actual.is_some_and(|actual| (actual - expected).abs() < EPSILON),
        "case {case:?}: expected {what} {expected}, got {actual:?}"
    );
}

/// The windows the rule says a read of `readings` at window length `window` must
/// report: the readings grouped by the epoch-aligned window the instant of each
/// falls in, oldest window first, as `(window start, readings of that window)`.
///
/// An INDEPENDENT oracle — it groups the instants this file wrote rather than
/// asking the service what it bucketed them into.
fn expected_windows(window: i64, readings: &[f64; 3]) -> Vec<(i64, Vec<f64>)> {
    let mut windows: Vec<(i64, Vec<f64>)> = Vec::new();
    for (offset, reading) in SAMPLE_OFFSETS.iter().zip(readings) {
        let start = grid_start() + offset - offset.rem_euclid(window);
        match windows.last_mut() {
            Some((last, values)) if *last == start => values.push(*reading),
            _ => windows.push((start, vec![*reading])),
        }
    }
    windows
}

/// `GET /properties/aggregates` for one property, over the whole day after
/// [`grid_start`], with `extra` appended to the query.
fn aggregates_uri(asset_id: &str, property_id: &str, resolution: &str, extra: &str) -> String {
    let types: String = AGGREGATES
        .iter()
        .map(|(requested, _)| format!("&aggregateTypes={requested}"))
        .collect();
    format!(
        "/properties/aggregates?assetId={asset_id}&propertyId={property_id}\
         &resolution={resolution}&startDate={}&endDate={}{types}{extra}",
        iso(grid_start()),
        iso(grid_start() + DAY)
    )
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

/// The id of the property `name` a composite model of `model_id` contributes, as
/// `DescribeAssetModelCompositeModel` reports it.
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

/// The property ids `ListAssetProperties` reports for `asset_id` over every
/// property it carries (`filter=ALL`) — the enumeration a client takes a property
/// id out of before reading that property's aggregates.
async fn listed_property_ids(router: &MultiServiceRouter, asset_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssetProperties: got {status} {body}");
    body["assetPropertySummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// An asset carrying a property in every place an asset carries one, answered as
/// `(assetId, rows)`.
///
/// The composite models after the first are created AFTER the asset, so the rows
/// cover a property that reached the asset through the propagation a
/// composite-model create performs as well as ones materialised with it.
///
/// `label` keeps the names of two fixtures in one test binary apart.
async fn press(router: &MultiServiceRouter, label: &str) -> (String, Vec<Row>) {
    // A COMPONENT_MODEL with a property of its own, reused below: a
    // component-model-based composite model stores no properties, and the
    // component model it names provides them.
    let (status, component) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{label}Gauge"),
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Measured", "dataType": "DOUBLE", "unit": "bar",
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

    // The asset model: one property of its own and one composite model declared
    // with it.
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{label}Press"),
            "assetModelProperties": [
                {"name": "Force", "dataType": "DOUBLE", "unit": "kN",
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

    let asset_id = create_asset(router, &format!("{label}Press-1"), &model_id).await;

    // Three more composite models, added to the model the asset ALREADY exists
    // from: a custom one, one nested inside it, and one that reuses the component
    // model.
    let shift = composite_id(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Shift",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Throughput", "dataType": "DOUBLE", "unit": "units/h",
                 "type": {"measurement": {}}},
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

    // The asset's own property, read back from the service rather than assumed.
    let (status, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    let force = property_id(&asset["assetProperties"], "Force");

    // The composite model declared with the asset model, as the model reports it.
    let (status, model) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    let alarm = model["assetModelCompositeModels"]
        .as_array()
        .unwrap_or_else(|| panic!("no composite models in {model}"))
        .iter()
        .find(|composite| composite["name"] == "Alarm")
        .unwrap_or_else(|| panic!("no composite model named Alarm in {model}"))
        .clone();
    let alarm_value = property_id(&alarm["properties"], "AlarmValue");

    let throughput = composite_property(router, &model_id, &shift, "Throughput").await;
    let cycles = composite_property(router, &model_id, &shift, "Cycles").await;
    let deep = composite_property(router, &model_id, &nested, "Deep").await;
    let measured = composite_property(router, &model_id, &reused, "Measured").await;

    // Distinct readings per row, so a handler that answered one row's series for
    // another would be caught rather than agreeing with itself. Whole numbers, so
    // an INTEGER row writes the same readings a DOUBLE one does.
    let declared: [(&'static str, String, &'static str); 6] = [
        ("the asset's own property", force, "DOUBLE"),
        (
            "a property of a composite model declared with the asset model",
            alarm_value,
            "DOUBLE",
        ),
        (
            "a property of a composite model added after the asset was created",
            throughput,
            "DOUBLE",
        ),
        (
            "an INTEGER property of a composite model added after the asset was created",
            cycles,
            "INTEGER",
        ),
        (
            "a property of a composite model nested inside one added after the asset",
            deep,
            "DOUBLE",
        ),
        (
            "a property the component model a composite model reuses contributes",
            measured,
            "DOUBLE",
        ),
    ];
    let rows = declared
        .into_iter()
        .enumerate()
        .map(|(at, (case, property_id, data_type))| Row {
            case,
            property_id,
            data_type,
            readings: [
                10.0 + 4.0 * at as f64,
                11.0 + 4.0 * at as f64,
                12.0 + 4.0 * at as f64,
            ],
        })
        .collect();
    (asset_id, rows)
}

/// Write this row's samples through `BatchPutAssetPropertyValue`.
async fn write_samples(router: &MultiServiceRouter, asset_id: &str, row: &Row) {
    let values: Vec<Value> = SAMPLE_OFFSETS
        .iter()
        .zip(row.readings)
        .map(|(offset, reading)| {
            json!({
                "value": variant(row.data_type, reading),
                "timestamp": {"timeInSeconds": grid_start() + offset},
                "quality": "GOOD",
            })
        })
        .collect();
    let (status, body) = call(
        router,
        "POST",
        BATCH_PUT,
        Some(json!({"entries": [{
            "entryId": "seed",
            "assetId": asset_id,
            "propertyId": row.property_id,
            "propertyValues": values,
        }]})),
    )
    .await;
    let case = row.case;
    assert_eq!(status, 200, "case {case:?}: the write got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: every sample of a property of this asset must be accepted, got {body}"
    );
}

/// The `aggregatedValues` of a 200 answer.
fn windows_of(case: &str, body: &Value) -> Vec<Value> {
    body["aggregatedValues"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {body}"))
        .clone()
}

#[tokio::test]
async fn the_aggregate_read_serves_every_property_the_asset_reports_at_every_resolution() {
    let router = router();
    let (asset_id, rows) = press(&router, "Window").await;

    let listed = listed_property_ids(&router, &asset_id).await;
    for row in &rows {
        let case = row.case;
        write_samples(&router, &asset_id, row).await;

        // The two reads that publish this pair AS a property of this asset: the
        // aggregate read below must answer the same pair.
        assert!(
            listed.contains(&row.property_id),
            "case {case:?}: ListAssetProperties must report {}, got {listed:?}",
            row.property_id
        );
        let (status, described) = call(
            &router,
            "GET",
            &format!("/assets/{asset_id}/properties/{}", row.property_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {described}"
        );

        for (resolution, window) in RESOLUTIONS {
            let uri = aggregates_uri(&asset_id, &row.property_id, resolution, "");
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{resolution}: GetAssetPropertyAggregates got {status} {body}"
            );
            let reported = windows_of(case, &body);
            let expected = expected_windows(window, &row.readings);
            assert_eq!(
                reported.len(),
                expected.len(),
                "case {case:?}/{resolution}: expected {} windows, got {body}",
                expected.len()
            );
            for (at, (start, readings)) in expected.iter().enumerate() {
                let value = &reported[at]["value"];
                let sum: f64 = readings.iter().sum();
                let count = readings.len() as f64;
                assert_close(
                    case,
                    &format!("the {resolution} window {at} end"),
                    reported[at]["timestamp"].as_f64(),
                    (*start + window) as f64,
                );
                assert_close(
                    case,
                    &format!("the {resolution} window {at} count"),
                    value["count"].as_f64(),
                    count,
                );
                assert_close(
                    case,
                    &format!("the {resolution} window {at} sum"),
                    value["sum"].as_f64(),
                    sum,
                );
                assert_close(
                    case,
                    &format!("the {resolution} window {at} minimum"),
                    value["minimum"].as_f64(),
                    readings.iter().copied().fold(f64::INFINITY, f64::min),
                );
                assert_close(
                    case,
                    &format!("the {resolution} window {at} maximum"),
                    value["maximum"].as_f64(),
                    readings.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                );
                assert_close(
                    case,
                    &format!("the {resolution} window {at} average"),
                    value["average"].as_f64(),
                    sum / count,
                );
            }

            // A read changes nothing a later caller sees: the same request
            // answers the same body.
            let (status, again) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{resolution}: the second read got {status} {again}"
            );
            assert_eq!(
                again, body,
                "case {case:?}/{resolution}: a read must not change what a later read reports"
            );
        }

        // A range the property holds no reading in is the EMPTY set of windows of
        // a property that exists, not a refusal.
        let (status, empty) = call(
            &router,
            "GET",
            &format!(
                "/properties/aggregates?assetId={asset_id}&propertyId={}\
                 &resolution=1h&aggregateTypes=AVERAGE&startDate={}&endDate={}",
                row.property_id,
                iso(grid_start() - 2 * 3_600),
                iso(grid_start() - 3_600)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a range with no reading got {status} {empty}"
        );
        assert_eq!(
            windows_of(case, &empty).len(),
            0,
            "case {case:?}: a range with no reading holds no window, got {empty}"
        );
    }

    // The boundary of the rule: a pair naming no property of the asset, and an
    // asset id naming no asset, are the modelled refusal rather than an answer.
    let first = &rows[0].property_id;
    let refusals: [(&str, String); 2] = [
        (
            "a property id that names no property of the asset",
            aggregates_uri(&asset_id, ABSENT_ID, "1h", ""),
        ),
        (
            "an asset id that names no asset",
            aggregates_uri(ABSENT_ID, first, "1h", ""),
        ),
    ];
    for (case, uri) in refusals {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled refusal, got {body}"
        );
    }

    // The refusal names what could not be resolved: the asset and the property
    // the request asked for.
    let (_, body) = call(
        &router,
        "GET",
        &aggregates_uri(&asset_id, ABSENT_ID, "1h", ""),
        None,
    )
    .await;
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains(&asset_id) && message.contains(ABSENT_ID),
        "the refusal must name the asset {asset_id} and the property {ABSENT_ID}, got {message:?}"
    );
}

#[tokio::test]
async fn the_aggregate_read_pages_and_orders_every_propertys_windows_alike() {
    let router = router();
    let (asset_id, rows) = press(&router, "Page").await;

    for row in &rows {
        let case = row.case;
        write_samples(&router, &asset_id, row).await;

        // One window per sample at the shortest resolution, so there is something
        // to page through and to order. Each window is stamped with the instant it
        // ends, so a `1m` window starting at `start` reports `start + 60`.
        let ascending: Vec<i64> = expected_windows(60, &row.readings)
            .into_iter()
            .map(|(start, _)| start + 60)
            .collect();
        let mut descending = ascending.clone();
        descending.reverse();

        let orderings: [(&str, &str, &Vec<i64>); 3] = [
            ("default ordering", "", &ascending),
            ("ASCENDING", "&timeOrdering=ASCENDING", &ascending),
            ("DESCENDING", "&timeOrdering=DESCENDING", &descending),
        ];
        for (ordering, parameter, expected) in orderings {
            for page_size in 1..=expected.len() + 1 {
                let mut collected: Vec<i64> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0;
                loop {
                    let resume = token
                        .as_deref()
                        .map(|token| format!("&nextToken={token}"))
                        .unwrap_or_default();
                    let uri = aggregates_uri(
                        &asset_id,
                        &row.property_id,
                        "1m",
                        &format!("{parameter}&maxResults={page_size}{resume}"),
                    );
                    let (status, body) = call(&router, "GET", &uri, None).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}/{ordering}/{page_size}: got {status} {body}"
                    );
                    let page = windows_of(case, &body);
                    assert!(
                        page.len() <= page_size,
                        "case {case:?}/{ordering}/{page_size}: a page carries at most \
                         maxResults windows, got {body}"
                    );
                    collected.extend(
                        page.iter()
                            .map(|window| window["timestamp"].as_f64().unwrap_or_default() as i64),
                    );
                    token = body["nextToken"].as_str().map(str::to_string);
                    pages += 1;
                    assert!(
                        pages <= expected.len() + 1,
                        "case {case:?}/{ordering}/{page_size}: the walk must terminate, \
                         collected {collected:?}"
                    );
                    if token.is_none() {
                        break;
                    }
                }
                assert_eq!(
                    &collected, expected,
                    "case {case:?}/{ordering}/{page_size}: expected the windows {expected:?}, \
                     got {collected:?}"
                );
            }
        }
    }
}
