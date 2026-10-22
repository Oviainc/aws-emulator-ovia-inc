// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two `forecastquery` (Amazon Forecast Runtime) operations.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /` through the
//! real [`MultiServiceRouter`], so the modelled constraints, the routing table, the
//! handler and the error envelope are all exercised.
//!
//! The forecasts under test are PERSISTED before the call, standing in for the Amazon
//! Forecast control plane (`CreateForecast` / `CreateWhatIfForecast`), which is the only
//! thing that mints a forecast ARN. Nothing here asserts a value the service invented:
//! each assertion compares the response against what was stored.

use std::collections::BTreeMap;
use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// A daily series: the timestamps every window case below slices.
const TIMESTAMPS: [&str; 6] = [
    "2015-01-01T00:00:00",
    "2015-01-02T00:00:00",
    "2015-01-03T00:00:00",
    "2015-01-04T00:00:00",
    "2015-01-05T00:00:00",
    "2015-01-06T00:00:00",
];

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // the operations, so it turns verification off rather than signing each request.
    router.set_request_authentication(false);
    router.register_service(forecastquery::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Persist a forecast under `ForecastArn`, as `CreateForecast` would.
fn seed_forecast(engine: &InMemoryEngine, forecast: StoredForecast) {
    engine.state::<ForecastQueryState>().lock().put_forecast(forecast);
}

/// Persist a what-if forecast under `WhatIfForecastArn`, as `CreateWhatIfForecast` would.
fn seed_what_if(engine: &InMemoryEngine, forecast: StoredForecast) {
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_what_if_forecast(forecast);
}

/// An ARN of the shape the control plane mints (and the only shape the model's
/// `^arn:([a-z\d-]+):forecast:.*:.*:.+$` pattern admits).
fn arn(resource: &str) -> String {
    engine::emulator_arn("forecast", "us-east-1", resource)
}

/// One awsJson1_1 call: `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonForecastRuntime.{operation}"))
        // The routing key: the SigV4 credential scope's signing name (`forecast`).
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAEXAMPLEEXAMPLE/20150101/us-east-1/forecast/aws4_request, \
             SignedHeaders=content-type;host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize request body"),
        ))
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `Forecast.Predictions` as `statistic -> [(timestamp, value)]`, ordered as it arrived.
///
/// A `BTreeMap` only because a JSON object's member order is not part of the protocol —
/// the per-statistic LIST order is, and is preserved here.
fn predictions(body: &Value) -> BTreeMap<String, Vec<(String, f64)>> {
    body.get("Forecast")
        .and_then(|forecast| forecast.get("Predictions"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(statistic, series)| {
                    let points = series
                        .as_array()
                        .expect("Predictions value must be a list")
                        .iter()
                        .map(|point| {
                            (
                                point["Timestamp"]
                                    .as_str()
                                    .expect("DataPoint.Timestamp must be a string")
                                    .to_string(),
                                point["Value"]
                                    .as_f64()
                                    .expect("DataPoint.Value must be a double"),
                            )
                        })
                        .collect();
                    (statistic.clone(), points)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The wire error code of a failure response: `__type` in the body, else the
/// `x-amzn-errortype` style member some envelopes use.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        // `__type` may be namespaced (`com.amazonaws.forecastquery#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A forecast whose single series carries `p10`/`p50`/`p90` over [`TIMESTAMPS`].
fn electricity_forecast(forecast_arn: &str) -> StoredForecast {
    let series = StoredSeries::new([("item_id", "client_21")])
        .with_statistic("p10", quantile_points(10.0))
        .with_statistic("p50", quantile_points(20.0))
        .with_statistic("p90", quantile_points(30.0));
    StoredForecast::new(
        forecast_arn,
        "electricity_demand",
        "D",
        ["0.1", "0.5", "0.9"],
    )
    .with_series(series)
}

/// Points over [`TIMESTAMPS`] starting at `base`, rising by 1.5 a day.
fn quantile_points(base: f64) -> Vec<StoredPoint> {
    TIMESTAMPS
        .iter()
        .enumerate()
        .map(|(day, timestamp)| StoredPoint::new(*timestamp, base + 1.5 * day as f64))
        .collect()
}

// ---------------------------------------------------------------------------
// The stored series is what comes back
// ---------------------------------------------------------------------------

/// A query answers with exactly the stored series of the statistic of every forecast type
/// the forecast was created with — over forecasts that differ in how many statistics they
/// carry, how many points each one has, and which dimensions identify the series.
#[tokio::test]
async fn predictions_are_the_stored_series_of_each_forecast_type() {
    struct Case {
        name: &'static str,
        forecast_types: Vec<&'static str>,
        /// The statistic keys the response must carry, spelled out rather than computed,
        /// so this test does not depend on the mapping it is not about.
        expected_statistics: Vec<&'static str>,
        dimensions: Vec<(&'static str, &'static str)>,
        stored: Vec<(&'static str, Vec<StoredPoint>)>,
        filters: Value,
    }

    let cases = vec![
        Case {
            name: "three quantiles, one dimension",
            forecast_types: vec!["0.1", "0.5", "0.9"],
            expected_statistics: vec!["p10", "p50", "p90"],
            dimensions: vec![("item_id", "client_21")],
            stored: vec![
                ("p10", quantile_points(10.0)),
                ("p50", quantile_points(20.0)),
                ("p90", quantile_points(30.0)),
            ],
            filters: json!({"item_id": "client_21"}),
        },
        Case {
            name: "mean only, two dimensions",
            forecast_types: vec!["mean"],
            expected_statistics: vec!["mean"],
            dimensions: vec![("item_id", "sku-9"), ("store", "seattle")],
            stored: vec![("mean", quantile_points(7.25))],
            filters: json!({"item_id": "sku-9", "store": "seattle"}),
        },
        Case {
            name: "single point, five statistics",
            forecast_types: vec!["0.01", "0.25", "0.5", "0.75", "0.99"],
            expected_statistics: vec!["p1", "p25", "p50", "p75", "p99"],
            dimensions: vec![("item_id", "one_shot")],
            stored: vec![
                ("p1", vec![StoredPoint::new(TIMESTAMPS[0], -3.5)]),
                ("p25", vec![StoredPoint::new(TIMESTAMPS[0], 0.0)]),
                ("p50", vec![StoredPoint::new(TIMESTAMPS[0], 1.0)]),
                ("p75", vec![StoredPoint::new(TIMESTAMPS[0], 2.25)]),
                ("p99", vec![StoredPoint::new(TIMESTAMPS[0], 9.75)]),
            ],
            filters: json!({"item_id": "one_shot"}),
        },
        Case {
            // A forecast type with no stored points for it is reported as the empty
            // series, not omitted and not filled in.
            name: "a forecast type the series holds no points for",
            forecast_types: vec!["0.5", "0.9"],
            expected_statistics: vec!["p50", "p90"],
            dimensions: vec![("item_id", "sparse")],
            stored: vec![("p50", quantile_points(1.0))],
            filters: json!({"item_id": "sparse"}),
        },
        Case {
            // Two forecast types naming one statistic are ONE reported series:
            // `Predictions` is keyed by statistic, so a duplicate cannot be two entries.
            name: "two forecast types naming the same statistic",
            forecast_types: vec!["0.5", "p50"],
            expected_statistics: vec!["p50"],
            dimensions: vec![("item_id", "collapsed")],
            stored: vec![("p50", quantile_points(3.0))],
            filters: json!({"item_id": "collapsed"}),
        },
    ];

    for case in cases {
        let (engine, router) = harness();
        let forecast_arn = arn(&format!("forecast/{}", case.name.replace(' ', "-")));
        let mut series = StoredSeries::new(case.dimensions.clone());
        for (statistic, points) in &case.stored {
            series = series.with_statistic(*statistic, points.clone());
        }
        seed_forecast(
            &engine,
            StoredForecast::new(&forecast_arn, "seeded", "D", case.forecast_types.clone())
                .with_series(series),
        );

        let (status, body) = call(
            &router,
            "QueryForecast",
            &json!({"ForecastArn": forecast_arn, "Filters": case.filters}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            case.name
        );

        let expected: BTreeMap<String, Vec<(String, f64)>> = case
            .expected_statistics
            .iter()
            .map(|statistic| {
                let points = case
                    .stored
                    .iter()
                    .find(|(name, _)| name == statistic)
                    .map(|(_, points)| {
                        points
                            .iter()
                            .map(|point| (point.timestamp.clone(), point.value))
                            .collect()
                    })
                    .unwrap_or_default();
                ((*statistic).to_string(), points)
            })
            .collect();
        let actual = predictions(&body);
        assert_eq!(
            actual, expected,
            "case {:?}: expected the stored series {expected:?}, got {actual:?}",
            case.name
        );
    }
}

/// A forecast type is reported as the statistic Amazon Forecast names it with: a quantile
/// `q` as `p{q*100}`, `mean` as `mean`.
///
/// Catalog source: `CreateForecast`'s `ForecastTypes` doc — "accepted values include 0.01
/// to 0.99 (increments of .01 only) and mean" — plus `Statistic`'s 4-character bound in
/// this service's model. Both published bounds (0.01, 0.99) are rows, as is a value past
/// each of them (0, 1), and a value the issue did not cite (0.07, where binary rounding
/// could have answered p6).
#[tokio::test]
async fn each_forecast_type_is_reported_as_its_statistic() {
    let catalog: [(&str, &str); 10] = [
        ("0.01", "p1"),
        ("0.07", "p7"),
        ("0.1", "p10"),
        ("0.5", "p50"),
        ("0.9", "p90"),
        ("0.99", "p99"),
        ("mean", "mean"),
        // Already a statistic name: reported unchanged.
        ("p10", "p10"),
        // Past the published bounds, so not a quantile: reported unchanged.
        ("0", "0"),
        ("1", "1"),
    ];

    for (forecast_type, statistic) in catalog {
        let (engine, router) = harness();
        let forecast_arn = arn(&format!("forecast/statistic-{statistic}"));
        let points = vec![StoredPoint::new(TIMESTAMPS[0], 42.0)];
        seed_forecast(
            &engine,
            StoredForecast::new(&forecast_arn, "statistics", "D", [forecast_type]).with_series(
                StoredSeries::new([("item_id", "client_21")])
                    .with_statistic(statistic, points.clone()),
            ),
        );

        let (status, body) = call(
            &router,
            "QueryForecast",
            &json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {forecast_type:?}: expected HTTP 200, got {status} with {body}"
        );
        let actual = predictions(&body);
        let keys: Vec<&String> = actual.keys().collect();
        assert_eq!(
            keys,
            vec![&statistic.to_string()],
            "case {forecast_type:?}: expected the single statistic {statistic:?}, got {keys:?}"
        );
        assert_eq!(
            actual[statistic],
            vec![(TIMESTAMPS[0].to_string(), 42.0)],
            "case {forecast_type:?}: expected the stored point, got {:?}",
            actual[statistic]
        );
    }
}

// ---------------------------------------------------------------------------
// The date window
// ---------------------------------------------------------------------------

/// `StartDate`/`EndDate` clip the stored points to the inclusive window; absent bounds
/// mean the complete date range.
#[tokio::test]
async fn the_date_window_clips_the_stored_points_inclusively() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/windowed");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));

    struct Case {
        name: &'static str,
        start: Option<&'static str>,
        end: Option<&'static str>,
        expected: Vec<&'static str>,
    }

    let cases = vec![
        Case {
            name: "no bounds: the complete date range",
            start: None,
            end: None,
            expected: TIMESTAMPS.to_vec(),
        },
        Case {
            name: "start on a stored point is inclusive",
            start: Some(TIMESTAMPS[2]),
            end: None,
            expected: TIMESTAMPS[2..].to_vec(),
        },
        Case {
            name: "end on a stored point is inclusive",
            start: None,
            end: Some(TIMESTAMPS[1]),
            expected: TIMESTAMPS[..2].to_vec(),
        },
        Case {
            name: "both bounds",
            start: Some(TIMESTAMPS[1]),
            end: Some(TIMESTAMPS[3]),
            expected: TIMESTAMPS[1..4].to_vec(),
        },
        Case {
            name: "a single instant",
            start: Some(TIMESTAMPS[4]),
            end: Some(TIMESTAMPS[4]),
            expected: vec![TIMESTAMPS[4]],
        },
        Case {
            name: "between two stored points",
            start: Some("2015-01-02T00:00:01"),
            end: Some("2015-01-02T23:59:59"),
            expected: vec![],
        },
        Case {
            name: "entirely after the series",
            start: Some("2016-01-01T00:00:00"),
            end: None,
            expected: vec![],
        },
        Case {
            name: "entirely before the series",
            start: None,
            end: Some("2014-12-31T23:59:59"),
            expected: vec![],
        },
    ];

    for case in cases {
        let mut request = json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "client_21"}});
        if let Some(start) = case.start {
            request["StartDate"] = json!(start);
        }
        if let Some(end) = case.end {
            request["EndDate"] = json!(end);
        }

        let (status, body) = call(&router, "QueryForecast", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            case.name
        );
        let actual = predictions(&body);
        for statistic in ["p10", "p50", "p90"] {
            let timestamps: Vec<&str> = actual[statistic]
                .iter()
                .map(|(timestamp, _)| timestamp.as_str())
                .collect();
            assert_eq!(
                timestamps, case.expected,
                "case {:?} / {statistic}: expected {:?}, got {timestamps:?}",
                case.name, case.expected
            );
        }
    }
}

/// Every window over the series is a SUBSET of the unbounded answer, ascending, and
/// answers the same on a repeat call.
///
/// The invariant is checked exhaustively over all 36 (start, end) pairs drawn from the
/// stored timestamps rather than over sampled random windows, so no case is left to
/// chance and nothing is written to disk.
#[tokio::test]
async fn every_window_is_an_ordered_idempotent_subset_of_the_whole_range() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/subset");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));

    let (status, body) = call(
        &router,
        "QueryForecast",
        &json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "client_21"}}),
    )
    .await;
    assert_eq!(status, 200, "the unbounded query must succeed, got {body}");
    let whole = predictions(&body);

    for start in TIMESTAMPS {
        for end in TIMESTAMPS {
            let case = (start, end);
            let request = json!({
                "ForecastArn": forecast_arn,
                "Filters": {"item_id": "client_21"},
                "StartDate": start,
                "EndDate": end,
            });
            let (status, body) = call(&router, "QueryForecast", &request).await;
            if start > end {
                // An inverted window is refused, not silently answered as empty.
                assert_eq!(
                    status, 400,
                    "case {case:?}: an inverted window must be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInputException",
                    "case {case:?}: expected InvalidInputException, got {body}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            let narrowed = predictions(&body);
            assert_eq!(
                narrowed.keys().collect::<Vec<_>>(),
                whole.keys().collect::<Vec<_>>(),
                "case {case:?}: narrowing must not change which statistics are reported, \
                 got {:?}",
                narrowed.keys().collect::<Vec<_>>()
            );
            for (statistic, points) in &narrowed {
                let full = &whole[statistic];
                assert!(
                    points.iter().all(|point| full.contains(point)),
                    "case {case:?} / {statistic}: expected a subset of {full:?}, got {points:?}"
                );
                assert!(
                    points.windows(2).all(|pair| pair[0].0 < pair[1].0),
                    "case {case:?} / {statistic}: expected ascending timestamps, got {points:?}"
                );
                let expected: Vec<&(String, f64)> = full
                    .iter()
                    .filter(|(timestamp, _)| {
                        timestamp.as_str() >= start && timestamp.as_str() <= end
                    })
                    .collect();
                let got: Vec<&(String, f64)> = points.iter().collect();
                assert_eq!(
                    got, expected,
                    "case {case:?} / {statistic}: expected {expected:?}, got {got:?}"
                );
            }

            // Idempotent: a read changes nothing, so the repeat answers identically.
            let (repeat_status, repeat_body) = call(&router, "QueryForecast", &request).await;
            assert_eq!(
                repeat_status, status,
                "case {case:?}: the repeated call changed status to {repeat_status}"
            );
            assert_eq!(
                predictions(&repeat_body),
                narrowed,
                "case {case:?}: the repeated call answered different predictions"
            );
        }
    }
}

/// A date member that is not `yyyy-MM-dd'T'HH:mm:ss` is refused with
/// `InvalidInputException`, and a calendar-valid one in that format is accepted.
#[tokio::test]
async fn a_date_outside_the_documented_format_is_refused() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/dates");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));

    let rejected: [&str; 8] = [
        "2015-01-01",                 // date only
        "2015-01-01T08:00",           // no seconds
        "2015-01-01T08:00:00Z",       // offset the format does not carry
        "2015-01-01 08:00:00",        // space instead of 'T'
        "2015-13-01T00:00:00",        // month past 12
        "2015-02-30T00:00:00",        // a day February never has
        "2015-01-01T25:00:00",        // hour past 23
        "tomorrow",
    ];
    for member in ["StartDate", "EndDate"] {
        for value in rejected {
            let mut request =
                json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "client_21"}});
            request[member] = json!(value);
            let (status, body) = call(&router, "QueryForecast", &request).await;
            assert_eq!(
                status, 400,
                "case {member}={value:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {member}={value:?}: expected InvalidInputException, got {body}"
            );
        }
    }

    // A leap day IS a valid date in that format, and is accepted.
    let (status, body) = call(
        &router,
        "QueryForecast",
        &json!({
            "ForecastArn": forecast_arn,
            "Filters": {"item_id": "client_21"},
            "StartDate": "2016-02-29T00:00:00",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case leap day: expected HTTP 200, got {status} with {body}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// `Filters` selects the stored series by its dimensions: a key that is not a dimension of
/// the forecast and a filter that does not name a single series are refused, and values
/// that match no series are a 404 — never a fabricated series.
#[tokio::test]
async fn filters_select_the_stored_series_by_its_dimensions() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/dimensions");
    // Two series sharing `item_id`, separated by the `store` forecast dimension.
    seed_forecast(
        &engine,
        StoredForecast::new(&forecast_arn, "by_store", "D", ["0.5"])
            .with_series(
                StoredSeries::new([("item_id", "sku-9"), ("store", "seattle")])
                    .with_statistic("p50", quantile_points(100.0)),
            )
            .with_series(
                StoredSeries::new([("item_id", "sku-9"), ("store", "austin")])
                    .with_statistic("p50", quantile_points(200.0)),
            ),
    );

    struct Case {
        name: &'static str,
        filters: Value,
        expect_status: u16,
        expect_code: &'static str,
        expect_first_value: Option<f64>,
    }

    let cases = vec![
        Case {
            name: "both dimensions name one series",
            filters: json!({"item_id": "sku-9", "store": "seattle"}),
            expect_status: 200,
            expect_code: "",
            expect_first_value: Some(100.0),
        },
        Case {
            name: "the other series",
            filters: json!({"item_id": "sku-9", "store": "austin"}),
            expect_status: 200,
            expect_code: "",
            expect_first_value: Some(200.0),
        },
        Case {
            name: "a filter matching two series does not name a single item",
            filters: json!({"item_id": "sku-9"}),
            expect_status: 400,
            expect_code: "InvalidInputException",
            expect_first_value: None,
        },
        Case {
            name: "a key that is not a dimension of this forecast",
            filters: json!({"item_id": "sku-9", "region": "emea"}),
            expect_status: 400,
            expect_code: "InvalidInputException",
            expect_first_value: None,
        },
        Case {
            name: "a known key whose value matches no series",
            filters: json!({"item_id": "sku-9", "store": "lisbon"}),
            expect_status: 404,
            expect_code: "ResourceNotFoundException",
            expect_first_value: None,
        },
        Case {
            name: "a known key whose item matches no series",
            filters: json!({"item_id": "sku-404"}),
            expect_status: 404,
            expect_code: "ResourceNotFoundException",
            expect_first_value: None,
        },
    ];

    for case in cases {
        let (status, body) = call(
            &router,
            "QueryForecast",
            &json!({"ForecastArn": forecast_arn, "Filters": case.filters}),
        )
        .await;
        assert_eq!(
            status, case.expect_status,
            "case {:?}: expected HTTP {}, got {status} with {body}",
            case.name, case.expect_status
        );
        if !case.expect_code.is_empty() {
            assert_eq!(
                error_code(&body),
                case.expect_code,
                "case {:?}: expected {}, got {body}",
                case.name,
                case.expect_code
            );
            assert!(
                body.get("Message")
                    .or_else(|| body.get("message"))
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {:?}: the error must carry a Message, got {body}",
                case.name
            );
        }
        if let Some(expected) = case.expect_first_value {
            let actual = predictions(&body);
            assert_eq!(
                actual["p50"].first().map(|(_, value)| *value),
                Some(expected),
                "case {:?}: expected the series stored for that dimension, got {actual:?}",
                case.name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Resolving the ARN
// ---------------------------------------------------------------------------

/// An ARN this service holds no forecast for is `ResourceNotFoundException` (404) in the
/// awsJson1_1 error shape — including an ARN that only the OTHER operation's store knows,
/// and one whose forecast was removed.
#[tokio::test]
async fn an_arn_no_forecast_is_stored_under_is_not_found() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/known");
    let what_if_arn = arn("what-if-forecast/known/variant");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));
    seed_what_if(
        &engine,
        electricity_forecast(&what_if_arn).derived_from(&forecast_arn),
    );

    let cases: Vec<(&str, &str, String)> = vec![
        // A well-formed ARN nothing was ever stored under.
        (
            "unknown forecast",
            "QueryForecast",
            arn("forecast/never-created"),
        ),
        (
            "unknown what-if forecast",
            "QueryWhatIfForecast",
            arn("what-if-forecast/never-created/variant"),
        ),
        // A what-if ARN is not a forecast ARN, and the reverse.
        (
            "what-if ARN through QueryForecast",
            "QueryForecast",
            what_if_arn.clone(),
        ),
        (
            "forecast ARN through QueryWhatIfForecast",
            "QueryWhatIfForecast",
            forecast_arn.clone(),
        ),
        // A different partition and region: still not an ARN this store holds.
        (
            "another partition",
            "QueryForecast",
            "arn:aws-cn:forecast:cn-north-1:000000000000:forecast/known".to_string(),
        ),
    ];

    for (name, operation, queried) in cases {
        let arn_member = if operation == "QueryForecast" {
            "ForecastArn"
        } else {
            "WhatIfForecastArn"
        };
        let (status, body) = call(
            &router,
            operation,
            &json!({arn_member: queried, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // Removing the forecast (as DeleteForecast would) turns its own ARN into a 404.
    engine
        .state::<ForecastQueryState>()
        .lock()
        .forecasts
        .remove(&forecast_arn);
    let (status, body) = call(
        &router,
        "QueryForecast",
        &json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "client_21"}}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"deleted forecast\": expected HTTP 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"deleted forecast\": expected ResourceNotFoundException, got {body}"
    );
}

/// A request that breaks a modelled constraint is refused with a 400 — never a 404, a 501
/// or a 200.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/constraints");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));

    // Every row is a 400. The code differs by WHAT was refused: a RESOURCE ARN outside
    // its modelled pattern or length is refused by the service itself, before the forecast
    // it names is resolved, with the operation's own modelled `InvalidInputException`
    // (`crate::service::override_refusal`); a `Filters` KEY pattern (a map-key constraint
    // the boundary does not walk) reaches the handler and is answered with the same
    // modelled error; every other modelled constraint is checked by the framework's
    // boundary validator before `handle` and keeps its generic `ValidationException`
    // report. Either way the caller gets a 400, never a 404 or a 501.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "ForecastArn that is not an ARN",
            json!({"ForecastArn": "electricity_demand", "Filters": {"item_id": "client_21"}}),
            "InvalidInputException",
        ),
        (
            "ForecastArn of another service",
            json!({
                "ForecastArn": "arn:aws:s3:us-east-1:000000000000:bucket/x",
                "Filters": {"item_id": "client_21"},
            }),
            "InvalidInputException",
        ),
        (
            "ForecastArn past its 256-character bound",
            json!({
                "ForecastArn": arn(&format!("forecast/{}", "x".repeat(256))),
                "Filters": {"item_id": "client_21"},
            }),
            "InvalidInputException",
        ),
        (
            "Filters below its 1-entry bound",
            json!({"ForecastArn": forecast_arn, "Filters": {}}),
            "ValidationException",
        ),
        (
            "a Filters key outside its character pattern",
            json!({"ForecastArn": forecast_arn, "Filters": {"item id!": "client_21"}}),
            "InvalidInputException",
        ),
        (
            "NextToken past its 3000-character bound",
            json!({
                "ForecastArn": forecast_arn,
                "Filters": {"item_id": "client_21"},
                "NextToken": "t".repeat(3001),
            }),
            "ValidationException",
        ),
        (
            "no Filters at all",
            json!({"ForecastArn": forecast_arn}),
            "ValidationException",
        ),
        (
            "no ForecastArn at all",
            json!({"Filters": {"item_id": "client_21"}}),
            "ValidationException",
        ),
    ];

    for (name, request, expected_code) in cases {
        let (status, body) = call(&router, "QueryForecast", &request).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {name:?}: expected {expected_code}, got {body}"
        );
    }
}

/// Neither operation issues a continuation token — the response shape has no member for
/// one — so a `NextToken` a caller supplies is never one this service minted, and is
/// refused with `InvalidNextTokenException` (400).
#[tokio::test]
async fn a_next_token_this_service_never_minted_is_invalid() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/tokens");
    let what_if_arn = arn("what-if-forecast/tokens/variant");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));
    seed_what_if(
        &engine,
        electricity_forecast(&what_if_arn).derived_from(&forecast_arn),
    );

    // One character (the modelled minimum), a plausible opaque handle, something shaped
    // like an echo of the request, and the modelled maximum length.
    let tokens: Vec<String> = vec![
        "x".to_string(),
        "eyJhcm4iOiJmb3JlY2FzdCIsIm9mZnNldCI6M30=".to_string(),
        forecast_arn.clone(),
        "t".repeat(3000),
    ];

    for token in tokens {
        for (operation, arn_member, queried) in [
            ("QueryForecast", "ForecastArn", &forecast_arn),
            ("QueryWhatIfForecast", "WhatIfForecastArn", &what_if_arn),
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    arn_member: queried,
                    "Filters": {"item_id": "client_21"},
                    "NextToken": token,
                }),
            )
            .await;
            let case = (operation, token.len());
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidNextTokenException",
                "case {case:?}: expected InvalidNextTokenException, got {body}"
            );
        }
    }

    // And no successful response offers a token to resume from.
    let (status, body) = call(
        &router,
        "QueryForecast",
        &json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "client_21"}}),
    )
    .await;
    assert_eq!(status, 200, "the untokenized query must succeed, got {body}");
    assert_eq!(
        body.as_object().map(|object| object.keys().len()),
        Some(1),
        "the response must carry only Forecast, got {body}"
    );
    assert!(
        body.get("NextToken").is_none() && body["Forecast"].get("NextToken").is_none(),
        "the modelled response has no NextToken member, got {body}"
    );
}

// ---------------------------------------------------------------------------
// What-if forecasts
// ---------------------------------------------------------------------------

/// A what-if forecast answers its OWN stored series, which differs from the base forecast
/// wherever the what-if transformation changed it — the same window and filter rules apply.
#[tokio::test]
async fn a_what_if_forecast_answers_its_own_transformed_series() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/electricity");
    let what_if_arn = arn("what-if-forecast/electricity/price-cut");

    let base = electricity_forecast(&forecast_arn);
    seed_forecast(&engine, base.clone());

    // The transformation the control plane's what-if analysis applied: a 25% uplift on
    // every point of every statistic. Computed here because CREATING the what-if forecast
    // is the control plane's job, not this runtime service's.
    const UPLIFT: f64 = 1.25;
    let base_series = &base.series[0];
    let mut transformed = StoredSeries::new([("item_id", "client_21")]);
    for (statistic, points) in &base_series.points {
        transformed = transformed.with_statistic(
            statistic,
            points
                .iter()
                .map(|point| StoredPoint::new(&point.timestamp, point.value * UPLIFT)),
        );
    }
    seed_what_if(
        &engine,
        StoredForecast::new(&what_if_arn, "price_cut", "D", ["0.1", "0.5", "0.9"])
            .with_series(transformed)
            .derived_from(&forecast_arn),
    );

    let filters = json!({"item_id": "client_21"});
    let (base_status, base_body) = call(
        &router,
        "QueryForecast",
        &json!({"ForecastArn": forecast_arn, "Filters": filters}),
    )
    .await;
    let (what_if_status, what_if_body) = call(
        &router,
        "QueryWhatIfForecast",
        &json!({"WhatIfForecastArn": what_if_arn, "Filters": filters}),
    )
    .await;
    assert_eq!(
        base_status, 200,
        "case \"base\": expected HTTP 200, got {base_status} with {base_body}"
    );
    assert_eq!(
        what_if_status, 200,
        "case \"what-if\": expected HTTP 200, got {what_if_status} with {what_if_body}"
    );

    let base_predictions = predictions(&base_body);
    let what_if_predictions = predictions(&what_if_body);
    assert_ne!(
        what_if_predictions, base_predictions,
        "case \"what-if\": the what-if series must not be the base forecast echoed back, \
         got {what_if_predictions:?}"
    );
    for (statistic, points) in &what_if_predictions {
        let expected: Vec<(String, f64)> = base_predictions[statistic]
            .iter()
            .map(|(timestamp, value)| (timestamp.clone(), value * UPLIFT))
            .collect();
        assert_eq!(
            points.len(),
            expected.len(),
            "case \"what-if\" / {statistic}: expected {expected:?}, got {points:?}"
        );
        // A tolerance, not bit equality: a `Value` is a double on the wire, so the
        // comparison must not depend on how it is rendered.
        for (got, want) in points.iter().zip(&expected) {
            assert_eq!(
                got.0, want.0,
                "case \"what-if\" / {statistic}: expected timestamp {:?}, got {:?}",
                want.0, got.0
            );
            assert!(
                (got.1 - want.1).abs() < 1e-9,
                "case \"what-if\" / {statistic} at {}: expected the stored transformed \
                 value {}, got {}",
                want.0,
                want.1,
                got.1
            );
        }
    }

    // The window and filter rules hold for the what-if operation too.
    let (status, body) = call(
        &router,
        "QueryWhatIfForecast",
        &json!({
            "WhatIfForecastArn": what_if_arn,
            "Filters": filters,
            "StartDate": TIMESTAMPS[2],
            "EndDate": TIMESTAMPS[3],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"what-if window\": expected HTTP 200, got {status} with {body}"
    );
    for (statistic, points) in predictions(&body) {
        let timestamps: Vec<&str> = points
            .iter()
            .map(|(timestamp, _)| timestamp.as_str())
            .collect();
        assert_eq!(
            timestamps,
            vec![TIMESTAMPS[2], TIMESTAMPS[3]],
            "case \"what-if window\" / {statistic}: expected the clipped window, got \
             {timestamps:?}"
        );
    }

    let (status, body) = call(
        &router,
        "QueryWhatIfForecast",
        &json!({"WhatIfForecastArn": what_if_arn, "Filters": {"region": "emea"}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"what-if unknown dimension\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case \"what-if unknown dimension\": expected InvalidInputException, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Error precedence
// ---------------------------------------------------------------------------

/// A resource ARN is READ before it is RESOLVED: an ARN outside its modelled pattern or
/// length is the modelled `InvalidInputException` (400), while an ARN that is well-formed
/// but names nothing is `ResourceNotFoundException` (404). Both operations, over a set of
/// malformed forms.
///
/// The malformed rows are forms the pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$` and the
/// length bound (`Arn` 0..256, `LongArn` 0..300) reject — the constraint source is
/// `smithy_query(query="closure",operation="QueryForecast,QueryWhatIfForecast")` — and
/// each is a DIFFERENT way of being outside them: not an ARN at all, another service's
/// ARN, an empty resource part, a partition outside the character class, and one past the
/// member's own maximum length. The well-formed rows differ only in naming nothing.
#[tokio::test]
async fn an_arn_outside_its_modelled_syntax_is_invalid_input_before_any_lookup() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/precedence");
    let what_if_arn = arn("what-if-forecast/precedence/variant");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));
    seed_what_if(
        &engine,
        electricity_forecast(&what_if_arn).derived_from(&forecast_arn),
    );

    /// One operation and the length its ARN member is bounded at.
    struct Operation {
        name: &'static str,
        arn_member: &'static str,
        max_length: usize,
    }

    let operations: [Operation; 2] = [
        Operation {
            name: "QueryForecast",
            arn_member: "ForecastArn",
            max_length: 256,
        },
        Operation {
            name: "QueryWhatIfForecast",
            arn_member: "WhatIfForecastArn",
            max_length: 300,
        },
    ];

    for operation in operations {
        let Operation {
            name,
            arn_member,
            max_length,
        } = operation;
        // (case, the ARN sent, expected status, expected code)
        let mut rows: Vec<(String, String, u16, &str)> = vec![
            (
                "not an ARN at all".to_string(),
                "definitely-not-an-arn".to_string(),
                400,
                "InvalidInputException",
            ),
            (
                "another service's ARN".to_string(),
                "arn:aws:s3:us-east-1:000000000000:bucket/x".to_string(),
                400,
                "InvalidInputException",
            ),
            (
                "an empty resource part".to_string(),
                "arn:aws:forecast:us-east-1:000000000000:".to_string(),
                400,
                "InvalidInputException",
            ),
            (
                "a partition outside the character class".to_string(),
                "arn:AWS:forecast:us-east-1:000000000000:forecast/x".to_string(),
                400,
                "InvalidInputException",
            ),
            (
                // Well-formed, and nothing is stored under it: resolution, not syntax.
                "well-formed but absent".to_string(),
                arn("forecast/absent-everywhere"),
                404,
                "ResourceNotFoundException",
            ),
        ];
        // One character past this member's own maximum length — a bound, so it is tested
        // at the bound (which must be accepted as syntax and resolve to a 404) and one
        // past it (refused as invalid input).
        let resource = "x".repeat(max_length);
        let at_bound = format!("arn:aws:forecast:us-east-1:000000000000:{resource}");
        let at_bound = at_bound[..max_length].to_string();
        rows.push((
            "exactly at the maximum length".to_string(),
            at_bound.clone(),
            404,
            "ResourceNotFoundException",
        ));
        rows.push((
            "one character past the maximum length".to_string(),
            format!("{at_bound}x"),
            400,
            "InvalidInputException",
        ));

        for (case, sent, expected_status, expected_code) in rows {
            let case = format!("{name} / {case}");
            let (status, body) = call(
                &router,
                name,
                &json!({
                    arn_member: sent,
                    "Filters": {"item_id": "client_21"},
                }),
            )
            .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
            assert!(
                body.get("Message")
                    .or_else(|| body.get("message"))
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the error must carry a Message, got {body}"
            );
        }
    }
}

/// Resolving the resource comes BEFORE judging a continuation token: a `NextToken` is a
/// position inside one resource's results, so an ARN that names nothing is
/// `ResourceNotFoundException` (404) even when the request also carries a token, while the
/// same token against a stored resource is `InvalidNextTokenException` (400).
#[tokio::test]
async fn resolving_the_resource_precedes_judging_a_next_token() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/token-precedence");
    let what_if_arn = arn("what-if-forecast/token-precedence/variant");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));
    seed_what_if(
        &engine,
        electricity_forecast(&what_if_arn).derived_from(&forecast_arn),
    );

    // Tokens differing in form and size, all inside the modelled 1..3000 bound.
    let tokens: [&str; 3] = [
        "x",
        "bogus-next-token",
        "eyJhcm4iOiJmb3JlY2FzdCIsIm9mZnNldCI6M30=",
    ];

    for (operation, arn_member, stored, absent) in [
        (
            "QueryForecast",
            "ForecastArn",
            forecast_arn.clone(),
            arn("forecast/never-created"),
        ),
        (
            "QueryWhatIfForecast",
            "WhatIfForecastArn",
            what_if_arn.clone(),
            arn("what-if-forecast/never-created/variant"),
        ),
    ] {
        for token in tokens {
            // (case, the ARN sent, expected status, expected code)
            let rows: [(&str, &String, u16, &str); 2] = [
                (
                    "an ARN that names nothing, plus a token",
                    &absent,
                    404,
                    "ResourceNotFoundException",
                ),
                (
                    "a stored resource, plus a token",
                    &stored,
                    400,
                    "InvalidNextTokenException",
                ),
            ];
            for (case, sent, expected_status, expected_code) in rows {
                let case = format!("{operation} / {case} / token {}", token.len());
                let (status, body) = call(
                    &router,
                    operation,
                    &json!({
                        arn_member: sent,
                        "Filters": {"item_id": "client_21"},
                        "NextToken": token,
                    }),
                )
                .await;
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected_code,
                    "case {case:?}: expected {expected_code}, got {body}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Which requests this service claims
// ---------------------------------------------------------------------------

/// This service's SigV4 signing name, `forecast`, is SHARED with the Amazon Forecast
/// CONTROL plane, so the signing name alone cannot tell the two apart: claiming on it
/// makes both services claim every Forecast-signed request and the router answers
/// neither. Both are awsJson1_1 posting to `/`, so the discriminator is the service the
/// request NAMES in `X-Amz-Target`.
///
/// With only this service mounted, a request it declines reaches no service at all and
/// the router answers `404 UnknownService` — which is the evidence wanted here: this
/// service does not claim the control plane's traffic. The control plane holds the
/// mirror table in
/// `services/service-com-amazonaws-forecast-amazonforecast/tests/all/request_claiming.rs`,
/// and the two together are the whole split: each claims only the targets naming it.
///
/// The rows differ along the service the target names and the spelling it uses. The
/// versioned spelling is a row because the framework's target reader strips a trailing
/// `_<digits>` API-version suffix, so both spellings name one service.
#[tokio::test]
async fn only_requests_naming_this_service_are_claimed() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/claiming");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));

    /// One awsJson1_1 `POST /` carrying `target` verbatim: `(status, wire error code)`.
    async fn dispatch(
        router: &MultiServiceRouter,
        target: &str,
        body: &Value,
    ) -> (u16, String) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAEXAMPLEEXAMPLE/20150101/us-east-1/forecast/aws4_request, \
                 SignedHeaders=content-type;host;x-amz-target, Signature=0",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize request body"),
            ))
            .expect("build request");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, error_code(&parsed))
    }

    let query = json!({"ForecastArn": forecast_arn, "Filters": {"item_id": "client_21"}});
    // (case, x-amz-target, expected status, expected error code)
    let rows: [(&str, &str, u16, &str); 5] = [
        ("its own QueryForecast", "AmazonForecastRuntime.QueryForecast", 200, ""),
        (
            "its own QueryForecast, versioned target spelling",
            "AmazonForecastRuntime_20180626.QueryForecast",
            200,
            "",
        ),
        (
            // The control plane's traffic: declined here, so with only this service
            // mounted the router finds no claimant.
            "the control plane's ListForecasts",
            "AmazonForecast.ListForecasts",
            404,
            "UnknownService",
        ),
        (
            "the control plane's CreateForecast",
            "AmazonForecast.CreateForecast",
            404,
            "UnknownService",
        ),
        (
            // A third service's target, signed for `forecast`: not this service's either.
            "another service entirely",
            "AWSKMS.Encrypt",
            404,
            "UnknownService",
        ),
    ];

    for (case, target, expected_status, expected_code) in rows {
        let (status, code) = dispatch(&router, target, &query).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?} ({target}): expected HTTP {expected_status} {expected_code:?}, \
             got {status} {code:?}"
        );
    }
}

/// Neither operation is a stub any more: no request answers HTTP 501.
#[tokio::test]
async fn neither_operation_answers_not_implemented() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/implemented");
    let what_if_arn = arn("what-if-forecast/implemented/variant");
    seed_forecast(&engine, electricity_forecast(&forecast_arn));
    seed_what_if(
        &engine,
        electricity_forecast(&what_if_arn).derived_from(&forecast_arn),
    );

    for (operation, arn_member, queried) in [
        ("QueryForecast", "ForecastArn", &forecast_arn),
        ("QueryWhatIfForecast", "WhatIfForecastArn", &what_if_arn),
    ] {
        let (status, body) = call(
            &router,
            operation,
            &json!({arn_member: queried, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected HTTP 200, got {status} with {body}"
        );
        assert!(
            !predictions(&body).is_empty(),
            "case {operation:?}: expected predictions from state, got {body}"
        );
    }
}
