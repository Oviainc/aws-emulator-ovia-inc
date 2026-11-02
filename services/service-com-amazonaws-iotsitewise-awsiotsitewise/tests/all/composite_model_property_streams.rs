// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise data plane addressed by
//! `(assetId, propertyId)`, driven over HTTP through the same `MultiServiceRouter`
//! the emulator boots with.
//!
//! The rule these suites pin: the properties of an asset are the ones its asset
//! model declares directly TOGETHER WITH the ones that model's composite models
//! contribute, and all of them are addressed by the same `(assetId, propertyId)`
//! pair — so every operation that names a property that way reaches the same
//! property, whichever of the two places the asset carries it in, and a pair that
//! names no property of the asset is still the modelled refusal.
//!
//! Each suite is a TABLE whose rows differ along WHERE the property is declared
//! (on the model itself, on a composite model created with the model, on one added
//! afterwards, on one nested inside another, and on the component model a
//! component-model-based composite model reuses) and along its data type, with the
//! asset's own property as the control row: every row must be served identically.
//!
//! The wire harness (`router`, `call`, `create_asset`, …) lives in the
//! `asset_lifecycle` suite, the composite-model creates in
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

/// Where `BatchGetAssetPropertyAggregates` is bound: `POST
/// /properties/batch/aggregates`.
const BATCH_AGGREGATES: &str = "/properties/batch/aggregates";

/// Where `BatchGetAssetPropertyValue` is bound: `POST /properties/batch/latest`.
const BATCH_LATEST: &str = "/properties/batch/latest";

/// Where `BatchGetAssetPropertyValueHistory` is bound: `POST
/// /properties/batch/history`.
const BATCH_HISTORY: &str = "/properties/batch/history";

/// An epoch second as the ISO-8601 `date-time` an `@httpQuery`-bound timestamp
/// travels as.
fn iso(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// How close two readings must be to count as the same number.
const EPSILON: f64 = 1e-9;

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The start of the `1h` aggregate window every row reads.
///
/// The service buckets a series on the epoch-aligned resolution grid, so an
/// hour-aligned instant is itself a window start. Taken from the shared anchor,
/// which sits inside the window `BatchPutAssetPropertyValue` ingests
/// (`[now - 7 days, now + 10 minutes]`) and is fixed once per test binary.
fn window_start() -> i64 {
    let anchor = super::support::anchor();
    anchor - anchor.rem_euclid(3_600)
}

/// How long that window is, in seconds — the `1h` resolution the rows ask for.
const WINDOW: i64 = 3_600;

/// The seconds, after [`window_start`], of the two samples every row writes.
const SAMPLE_OFFSETS: [i64; 2] = [10, 20];

/// One row of the table: a property of the asset under test, and where it is
/// declared.
pub(crate) struct Row {
    /// What the row varies: the case name every assertion carries.
    pub(crate) case: &'static str,
    /// The stored `propertyId` the row addresses.
    pub(crate) property_id: String,
    /// The property's declared `PropertyDataType`, which decides which `Variant`
    /// member a written sample carries.
    pub(crate) data_type: &'static str,
    /// The two readings this row writes, oldest first.
    pub(crate) readings: [f64; 2],
}

impl Row {
    /// The sum of this row's readings — what a window covering both reduces to.
    fn sum(&self) -> f64 {
        self.readings[0] + self.readings[1]
    }
}

/// The `Variant` carrying `reading` for a property of `data_type`.
pub(crate) fn variant(data_type: &str, reading: f64) -> Value {
    match data_type {
        "INTEGER" => json!({"integerValue": reading as i64}),
        _ => json!({"doubleValue": reading}),
    }
}

/// The numeric reading of a `Variant`, whichever member carries it.
pub(crate) fn numeric(value: &Value) -> Option<f64> {
    value
        .get("doubleValue")
        .and_then(Value::as_f64)
        .or_else(|| value.get("integerValue").and_then(Value::as_f64))
}

/// Assert `actual` is within [`EPSILON`] of `expected`.
pub(crate) fn assert_close(case: &str, what: &str, actual: Option<f64>, expected: f64) {
    assert!(
        actual.is_some_and(|actual| (actual - expected).abs() < EPSILON),
        "case {case:?}: expected {what} {expected}, got {actual:?}"
    );
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

/// The property ids a composite model of `model_id` contributes, as
/// `DescribeAssetModelCompositeModel` reports them.
pub(crate) async fn composite_property(
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

/// An asset whose properties sit in every place an asset carries one, answered as
/// `(assetId, rows)` with one row per property under test.
///
/// `label` keeps the names of two fixtures in one test binary apart.
pub(crate) async fn boiler(router: &MultiServiceRouter, label: &str) -> (String, Vec<Row>) {
    // A COMPONENT_MODEL with a property of its own, reused below: a
    // component-model-based composite model stores no properties, and the
    // component model it names provides them.
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
    assert_eq!(
        status, 202,
        "create component model: got {status} {component}"
    );
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

    // Three more composite models, added after the model was created: a custom
    // one, one nested inside it, and one that reuses the component model.
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
        .find(|composite| composite["name"] == "Alarm")
        .unwrap_or_else(|| panic!("no composite model named Alarm in {model}"))
        .clone();
    let alarm_value = property_id(&alarm["properties"], "AlarmValue");

    let throughput = composite_property(router, &model_id, &shift, "Throughput").await;
    let cycles = composite_property(router, &model_id, &shift, "Cycles").await;
    let deep = composite_property(router, &model_id, &nested, "Deep").await;
    let measured = composite_property(router, &model_id, &reused, "Measured").await;

    // Distinct readings per row, so a handler that answered one row's series for
    // another would be caught rather than agreeing with itself.
    let declared: [(&'static str, String, &'static str); 6] = [
        ("the asset's own property", temperature, "DOUBLE"),
        (
            "a property of a composite model declared with the asset model",
            alarm_value,
            "DOUBLE",
        ),
        (
            "a property of a composite model added to the model afterwards",
            throughput,
            "DOUBLE",
        ),
        (
            "an INTEGER property of a composite model",
            cycles,
            "INTEGER",
        ),
        (
            "a property of a composite model nested inside another",
            deep,
            "DOUBLE",
        ),
        (
            "a property a component model contributes through the composite model that reuses it",
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
            readings: [10.0 + 2.0 * at as f64, 20.0 + 2.0 * at as f64],
        })
        .collect();
    (asset_id, rows)
}

/// Write this row's two samples through `BatchPutAssetPropertyValue`.
pub(crate) async fn write_samples(router: &MultiServiceRouter, asset_id: &str, row: &Row) {
    let values: Vec<Value> = SAMPLE_OFFSETS
        .iter()
        .zip(row.readings)
        .map(|(offset, reading)| {
            json!({
                "value": variant(row.data_type, reading),
                "timestamp": {"timeInSeconds": window_start() + offset},
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

/// The one success entry of a batch answer, failing when the batch reported
/// anything else.
fn only_success(case: &str, body: &Value) -> Value {
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: expected no error entries, got {body}"
    );
    let entries = body["successEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no successEntries in {body}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected exactly one success entry, got {body}"
    );
    entries[0].clone()
}

#[tokio::test]
async fn every_data_plane_read_serves_a_composite_models_property_like_the_assets_own() {
    let router = router();
    let (asset_id, rows) = boiler(&router, "Stream").await;

    for row in &rows {
        let case = row.case;
        write_samples(&router, &asset_id, row).await;

        // The sibling read that already answered this pair: it must keep doing so,
        // and the data-plane reads below must agree with it about which property
        // the pair names.
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

        // BatchGetAssetPropertyAggregates: one window covering both samples.
        let aggregate_entry = json!({
            "entryId": "agg",
            "assetId": asset_id,
            "propertyId": row.property_id,
            "aggregateTypes": ["COUNT", "SUM", "MINIMUM", "MAXIMUM"],
            "resolution": "1h",
            "startDate": window_start(),
            "endDate": window_start() + WINDOW,
        });
        let (status, body) = call(
            &router,
            "POST",
            BATCH_AGGREGATES,
            Some(json!({"entries": [aggregate_entry.clone()]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAssetPropertyAggregates got {status} {body}"
        );
        let success = only_success(case, &body);
        let windows = success["aggregatedValues"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {success}"))
            .clone();
        assert_eq!(
            windows.len(),
            1,
            "case {case:?}: expected one window, got {success}"
        );
        // A window is stamped with the instant it ends: its start plus the length
        // of the `1h` window the entry asked for.
        assert_close(
            case,
            "the window timestamp",
            windows[0]["timestamp"].as_f64(),
            (window_start() + WINDOW) as f64,
        );
        assert_close(case, "count", windows[0]["value"]["count"].as_f64(), 2.0);
        assert_close(case, "sum", windows[0]["value"]["sum"].as_f64(), row.sum());
        assert_close(
            case,
            "minimum",
            windows[0]["value"]["minimum"].as_f64(),
            row.readings[0],
        );
        assert_close(
            case,
            "maximum",
            windows[0]["value"]["maximum"].as_f64(),
            row.readings[1],
        );

        // A read changes nothing a later caller sees: the same request answers the
        // same body.
        let (status, again) = call(
            &router,
            "POST",
            BATCH_AGGREGATES,
            Some(json!({"entries": [aggregate_entry]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the second batch read got {status} {again}"
        );
        assert_eq!(
            again, body,
            "case {case:?}: a read must not change what a later read reports"
        );

        // GetAssetPropertyAggregates: the single-property read of the same windows.
        let (status, single) = call(
            &router,
            "GET",
            &format!(
                "/properties/aggregates?assetId={asset_id}&propertyId={}\
                 &aggregateTypes=COUNT&aggregateTypes=SUM&resolution=1h\
                 &startDate={}&endDate={}",
                row.property_id,
                iso(window_start()),
                iso(window_start() + WINDOW)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAssetPropertyAggregates got {status} {single}"
        );
        let single_windows = single["aggregatedValues"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {single}"))
            .clone();
        assert_eq!(
            single_windows.len(),
            1,
            "case {case:?}: expected one window, got {single}"
        );
        assert_close(
            case,
            "the single-read sum",
            single_windows[0]["value"]["sum"].as_f64(),
            row.sum(),
        );

        // GetAssetPropertyValue and BatchGetAssetPropertyValue: the latest sample.
        let (status, latest) = call(
            &router,
            "GET",
            &format!(
                "/properties/latest?assetId={asset_id}&propertyId={}",
                row.property_id
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAssetPropertyValue got {status} {latest}"
        );
        assert_close(
            case,
            "the latest reading",
            numeric(&latest["propertyValue"]["value"]),
            row.readings[1],
        );
        let (status, body) = call(
            &router,
            "POST",
            BATCH_LATEST,
            Some(json!({"entries": [{
                "entryId": "latest",
                "assetId": asset_id,
                "propertyId": row.property_id,
            }]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAssetPropertyValue got {status} {body}"
        );
        let success = only_success(case, &body);
        assert_close(
            case,
            "the batched latest reading",
            numeric(&success["assetPropertyValue"]["value"]),
            row.readings[1],
        );

        // GetAssetPropertyValueHistory and BatchGetAssetPropertyValueHistory: both
        // samples, oldest first.
        let (status, history) = call(
            &router,
            "GET",
            &format!(
                "/properties/history?assetId={asset_id}&propertyId={}",
                row.property_id
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAssetPropertyValueHistory got {status} {history}"
        );
        let reported: Vec<Option<f64>> = history["assetPropertyValueHistory"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {history}"))
            .iter()
            .map(|sample| numeric(&sample["value"]))
            .collect();
        assert_eq!(
            reported.len(),
            2,
            "case {case:?}: expected both samples, got {history}"
        );
        assert_close(case, "the first sample", reported[0], row.readings[0]);
        assert_close(case, "the second sample", reported[1], row.readings[1]);
        let (status, body) = call(
            &router,
            "POST",
            BATCH_HISTORY,
            Some(json!({"entries": [{
                "entryId": "hist",
                "assetId": asset_id,
                "propertyId": row.property_id,
            }]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAssetPropertyValueHistory got {status} {body}"
        );
        let success = only_success(case, &body);
        let batched: Vec<Option<f64>> = success["assetPropertyValueHistory"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {success}"))
            .iter()
            .map(|sample| numeric(&sample["value"]))
            .collect();
        assert_eq!(
            batched, reported,
            "case {case:?}: the batched history must report what the single read did, got {success}"
        );

        // GetInterpolatedAssetPropertyValues: the interval grid starts at the
        // request's start, so a point asked for at a sample's own instant carries
        // that sample's reading.
        let (status, interpolated) = call(
            &router,
            "GET",
            &format!(
                "/properties/interpolated?assetId={asset_id}&propertyId={}\
                 &startTimeInSeconds={}&endTimeInSeconds={}&intervalInSeconds={}\
                 &quality=GOOD&type=LINEAR_INTERPOLATION",
                row.property_id,
                window_start() + SAMPLE_OFFSETS[0],
                window_start() + SAMPLE_OFFSETS[1],
                SAMPLE_OFFSETS[1] - SAMPLE_OFFSETS[0],
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetInterpolatedAssetPropertyValues got {status} {interpolated}"
        );
        let points: Vec<Option<f64>> = interpolated["interpolatedAssetPropertyValues"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: no interpolatedAssetPropertyValues in {interpolated}")
            })
            .iter()
            .map(|point| numeric(&point["value"]))
            .collect();
        assert_eq!(
            points.len(),
            2,
            "case {case:?}: expected a point per interval, got {interpolated}"
        );
        assert_close(case, "the first point", points[0], row.readings[0]);
        assert_close(case, "the second point", points[1], row.readings[1]);
    }
}

#[tokio::test]
async fn a_pair_that_names_no_property_of_the_asset_is_still_the_modelled_refusal() {
    let router = router();
    let (asset_id, rows) = boiler(&router, "Refusal").await;
    // A SECOND asset of its own model, carrying its own composite model property:
    // a property id that is somebody else's is not this asset's.
    let (other_asset, other_rows) = boiler(&router, "Other").await;
    let other_composite = other_rows
        .iter()
        .find(|row| row.case.contains("composite model"))
        .map(|row| row.property_id.clone())
        .expect("a composite model's property on the other asset");
    let own_composite = rows
        .iter()
        .find(|row| row.case.contains("composite model"))
        .map(|row| row.property_id.clone())
        .expect("a composite model's property on this asset");

    // Rows span the ways a pair names no property of the addressed asset: an id
    // nothing minted, a composite model's property of ANOTHER asset, and this
    // asset's own composite property read against that other asset.
    let cases = [
        ("an id no create minted", asset_id.clone(), ABSENT_ID.to_string()),
        (
            "another asset's composite model property",
            asset_id.clone(),
            other_composite,
        ),
        (
            "this asset's composite model property against another asset",
            other_asset.clone(),
            own_composite,
        ),
    ];

    for (case, asset, property) in cases {
        // The batch read reports it as an error entry, never as a success.
        let (status, body) = call(
            &router,
            "POST",
            BATCH_AGGREGATES,
            Some(json!({"entries": [{
                "entryId": "e1",
                "assetId": asset,
                "propertyId": property,
                "aggregateTypes": ["AVERAGE"],
                "resolution": "1h",
                "startDate": window_start(),
                "endDate": window_start() + WINDOW,
            }]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAssetPropertyAggregates got {status} {body}"
        );
        let errors = body["errorEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no errorEntries in {body}"))
            .clone();
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected one error entry, got {body}"
        );
        assert_eq!(
            errors[0]["errorCode"], "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["successEntries"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a refused entry must not also succeed, got {body}"
        );

        // The single-property reads refuse it with the modelled 404.
        for (what, uri) in [
            (
                "GetAssetPropertyValue",
                format!("/properties/latest?assetId={asset}&propertyId={property}"),
            ),
            (
                "GetAssetPropertyValueHistory",
                format!("/properties/history?assetId={asset}&propertyId={property}"),
            ),
            (
                "DescribeAssetProperty",
                format!("/assets/{asset}/properties/{property}"),
            ),
        ] {
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: {what} expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: {what} unexpected error code in {body}"
            );
        }
    }
}
