// SPDX-License-Identifier: CC0-1.0

//! How a query refuses a `Filters` key that is not a dimension of the queried forecast.
//!
//! The rule pinned here is one sentence for the whole class:
//!
//! > When a refusal's message names ONE offending entry of a map member, it picks that
//! > entry by a total order over the entries — the smallest key — and never by the map's
//! > own iteration order, which no wire order defines; so a byte-identical request
//! > against identical state always earns a byte-identical message, however many
//! > offending entries it carries and in whatever order it sent them.
//!
//! `Filters` is a map of 1..50 entries, so a request may carry several keys that are not
//! dimensions of the forecast. The rows below therefore differ along what the rule
//! generalises over: HOW MANY offending keys the map carries (one, two, six), which
//! character class they sort by (upper case against lower, a digit against a letter, a
//! key that is a prefix of another), their SIZE, whether admissible keys are mixed in,
//! and the ORDER they were sent in on the wire. Every row is sent repeatedly, because a
//! message picked out of a hash order is free to differ between two identical calls in
//! one process.
//!
//! Every row drives the service the way a client does: an awsJson1_1 `POST /` through the
//! real router, so the boundary validator, the handler and the error envelope all run,
//! and through BOTH query operations, which render the same shared outcome.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::Value;
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// How many times each row is sent. A map's iteration order is unspecified and differs
/// between two maps, so one call cannot tell a fixed choice from a lucky one; a handful
/// of identical calls can.
const REPEATS: usize = 8;

/// The dimensions the seeded forecast's one series carries, plus the item key — the keys
/// `Filters` may name, as the refusal lists them: sorted, because that is the only order
/// a map's key set has.
const VALID_KEYS: [&str; 3] = ["item_id", "region", "store"];

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // what a request is refused with, so it turns verification off rather than signing.
    router.set_request_authentication(false);
    router.register_service(forecastquery::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// An ARN of the shape the control plane mints.
fn arn(resource: &str) -> String {
    engine::emulator_arn("forecast", "us-east-1", resource)
}

/// A forecast with one series carrying every key in [`VALID_KEYS`], and one statistic.
fn stored_forecast(forecast_arn: &str) -> StoredForecast {
    let series = StoredSeries::new([
        ("item_id", "sku_9"),
        ("region", "emea"),
        ("store", "seattle"),
    ])
    .with_statistic("p50", [StoredPoint::new("2015-01-01T00:00:00", 42.0)]);
    StoredForecast::new(forecast_arn, "by_store", "D", ["0.5"]).with_series(series)
}

/// One awsJson1_1 call, with the body as it goes on the WIRE so a map's entry order is
/// this suite's to choose: `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &str) -> (u16, Value) {
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
        .body(bytes::Bytes::from(body.to_string()))
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

/// The wire error code of a failure response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` of a failure response, in either casing the protocols use.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A request body with `Filters` written in the order given, so the ENTRY ORDER on the
/// wire is part of the case.
fn request_body(arn_member: &str, queried: &str, filters: &[(String, String)]) -> String {
    let entries: Vec<String> = filters
        .iter()
        .map(|(key, value)| format!("{}:{}", Value::from(key.as_str()), Value::from(value.as_str())))
        .collect();
    format!(
        "{{{}:{},\"Filters\":{{{}}}}}",
        Value::from(arn_member),
        Value::from(queried),
        entries.join(",")
    )
}

/// The message a key that is not a dimension of the forecast earns.
fn unknown_key_message(key: &str) -> String {
    format!(
        "Filters key '{key}' is not a dimension of this forecast; valid keys are [{}]",
        VALID_KEYS.join(", ")
    )
}

/// The two operations, each with the ARN member it resolves a resource by and an ARN of
/// its own kind the store holds a forecast under.
struct Op {
    name: &'static str,
    arn_member: &'static str,
    stored_arn: String,
}

fn operations() -> [Op; 2] {
    [
        Op {
            name: "QueryForecast",
            arn_member: "ForecastArn",
            stored_arn: arn("forecast/by_store"),
        },
        Op {
            name: "QueryWhatIfForecast",
            arn_member: "WhatIfForecastArn",
            stored_arn: arn("what-if-forecast/by_store/price_cut"),
        },
    ]
}

/// Seed one forecast and one what-if forecast, as the control plane's create operations
/// would, under the ARNs [`operations`] names.
fn seed(engine: &InMemoryEngine) {
    let [forecast, what_if] = operations();
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_forecast(stored_forecast(&forecast.stored_arn));
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_what_if_forecast(
            stored_forecast(&what_if.stored_arn).derived_from(&forecast.stored_arn),
        );
}

/// One row: the `Filters` entries to send, and the key the refusal must name.
struct Case {
    name: &'static str,
    filters: Vec<(String, String)>,
    expect_key: String,
}

/// `Filters` entries from the pairs a row lists.
fn entries(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn cases() -> Vec<Case> {
    let long_key = "k".repeat(200);
    vec![
        Case {
            name: "one key that is not a dimension",
            filters: entries(&[("item_id", "sku_9"), ("colour", "red")]),
            expect_key: "colour".to_string(),
        },
        Case {
            name: "two keys, neither a dimension",
            filters: entries(&[("alpha_key", "1"), ("beta_key", "2")]),
            expect_key: "alpha_key".to_string(),
        },
        Case {
            name: "six keys, none a dimension",
            filters: entries(&[
                ("mike", "1"),
                ("zulu", "2"),
                ("kilo", "3"),
                ("delta", "4"),
                ("echo", "5"),
                ("tango", "6"),
            ]),
            expect_key: "delta".to_string(),
        },
        Case {
            name: "dimensions mixed in with keys that are not dimensions",
            filters: entries(&[
                ("store", "seattle"),
                ("zulu", "1"),
                ("item_id", "sku_9"),
                ("mike", "2"),
                ("region", "emea"),
            ]),
            expect_key: "mike".to_string(),
        },
        Case {
            // Key order is byte order, so an upper-case initial sorts ahead of a
            // lower-case one.
            name: "an upper-case key beside a lower-case one",
            filters: entries(&[("alpha", "1"), ("Zeta", "2")]),
            expect_key: "Zeta".to_string(),
        },
        Case {
            name: "a key starting with a digit beside one starting with a letter",
            filters: entries(&[("aa", "1"), ("9x", "2")]),
            expect_key: "9x".to_string(),
        },
        Case {
            name: "a key that is a prefix of another key",
            filters: entries(&[("ab_c", "1"), ("ab", "2")]),
            expect_key: "ab".to_string(),
        },
        Case {
            // Size is not order: a 200-character key sorts behind a short one that is
            // smaller byte for byte.
            name: "a long key beside a short one",
            filters: vec![(long_key, "1".to_string()), ("aaa".to_string(), "2".to_string())],
            expect_key: "aaa".to_string(),
        },
    ]
}

/// A `Filters` key that is not a dimension of the queried forecast is HTTP 400
/// `InvalidInputException` naming the SMALLEST such key and listing the forecast's valid
/// keys — the same message for every repetition of the request and whichever order the
/// entries were sent in.
#[tokio::test]
async fn the_refusal_names_the_smallest_key_that_is_not_a_dimension() {
    let (engine, router) = harness();
    seed(&engine);

    for op in operations() {
        for case in cases() {
            let mut reversed = case.filters.clone();
            reversed.reverse();
            for (order, filters) in [("as sent", &case.filters), ("reversed", &reversed)] {
                let body = request_body(op.arn_member, &op.stored_arn, filters);
                let expected = unknown_key_message(&case.expect_key);
                for repeat in 0..REPEATS {
                    let label = (op.name, case.name, order, repeat);
                    let (status, response) = call(&router, op.name, &body).await;
                    assert_eq!(
                        status, 400,
                        "case {label:?}: expected HTTP 400, got {status} with {response}"
                    );
                    assert_eq!(
                        error_code(&response),
                        "InvalidInputException",
                        "case {label:?}: expected InvalidInputException, got {response}"
                    );
                    assert_eq!(
                        error_message(&response),
                        expected,
                        "case {label:?}: expected the refusal to name {:?}, got {:?}",
                        case.expect_key,
                        error_message(&response)
                    );
                }
            }
        }
    }
}

/// The same rule read the other way round: a `Filters` map whose keys ARE all dimensions
/// of the forecast is not refused at all, so the message above is about the offending
/// keys and not about carrying several keys.
#[tokio::test]
async fn filters_whose_keys_are_all_dimensions_are_answered() {
    let (engine, router) = harness();
    seed(&engine);

    let accepted: [(&str, Vec<(String, String)>); 3] = [
        ("the item key alone", entries(&[("item_id", "sku_9")])),
        (
            "two dimensions",
            entries(&[("item_id", "sku_9"), ("region", "emea")]),
        ),
        (
            "every dimension, in reverse key order",
            entries(&[
                ("store", "seattle"),
                ("region", "emea"),
                ("item_id", "sku_9"),
            ]),
        ),
    ];

    for op in operations() {
        for (name, filters) in &accepted {
            let label = (op.name, *name);
            let body = request_body(op.arn_member, &op.stored_arn, filters);
            let (status, response) = call(&router, op.name, &body).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200, got {status} with {response}"
            );
            let points = response
                .pointer("/Forecast/Predictions/p50")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                points, 1,
                "case {label:?}: expected the series' one p50 point, got {response}"
            );
        }
    }
}
