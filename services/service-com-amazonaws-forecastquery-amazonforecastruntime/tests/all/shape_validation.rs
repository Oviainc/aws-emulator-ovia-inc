// SPDX-License-Identifier: CC0-1.0

//! The SHAPE-level validation of a query request: what a request value is refused with
//! BEFORE the service resolves anything.
//!
//! Two rules, each asserted over a set of inputs rather than one request:
//!
//! 1. A `DateTime` member (`StartDate`, `EndDate`) whose value is outside the pattern the
//!    shape layer judges it by (`crate::state::DATE_TIME_PATTERN`) is refused with HTTP
//!    400 `ValidationException`, whose message is the envelope AWS words a shape refusal
//!    in — naming the member in lowerCamel, quoting the value sent and stating the
//!    constraint violated — and that refusal does not depend on the ARN resolving.
//! 2. The same layer judges the KEYS of `Filters` against the key shape's `@length` and
//!    `@pattern` (`AttributeName`: 0..256, `^[a-zA-Z0-9\_\-]+$` —
//!    `smithy_query(query="shape",shape="com.amazonaws.forecastquery#Filters")`),
//!    reporting the whole map at the map member with the key shape's whole constraint set
//!    as the requirement, because the constraint belongs to every key of the map.
//!
//! Neither constraint is one the framework's boundary walk enforces: the on-disk
//! `DateTime` carries no `@pattern` at all, and the walk captures a map KEY as a path
//! segment and deliberately never judges it. So both are stated by this service, in the
//! same envelope the boundary layer words a refusal in and with the same precedence over
//! the RESOURCE: ahead of resolving it and of judging a `NextToken` against it, and
//! behind reading the ARN member itself, whose own answer `invalid_arn_message.rs` pins.
//!
//! How an ARN this service cannot read, and a resource it cannot find, are WORDED is a
//! different rule, pinned by `invalid_arn_message.rs` and `resource_not_found_message.rs`;
//! this suite only relies on them to show that a shape refusal comes first.
//!
//! Every row drives the service the way a client does: an awsJson1_1 `POST /` through the
//! real router, so the boundary validator, the handler and the error envelope all run.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// The pattern a `DateTime` member is judged by. Pinned here as a literal: it is the
/// pattern the real service quotes when it refuses a date, so a change to it is a change
/// to the rule rather than a refactor.
const DATE_TIME_PATTERN: &str = r"^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}[a-zA-Z0-9\:\+\-\[\]\.\/]*$";

/// The requirement a `Filters` key must satisfy, bundled as the shape layer bundles it:
/// `AttributeName`'s `@length` (0..256) and its `@pattern`.
const FILTER_KEY_REQUIREMENT: &str = "Map keys must satisfy constraint: [Member must have \
     length less than or equal to 256, Member must have length greater than or equal to \
     0, Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\_\\-]+$]";

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

/// A forecast with one series (`item_id=client_21`) and one statistic.
fn stored_forecast(forecast_arn: &str) -> StoredForecast {
    let series = StoredSeries::new([("item_id", "client_21")]).with_statistic(
        "p50",
        [StoredPoint::new("2015-01-01T00:00:00", 42.0)],
    );
    StoredForecast::new(forecast_arn, "electricity_demand", "D", ["0.5"]).with_series(series)
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

/// The one-violation envelope a shape refusal is reported in.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a value outside the `DateTime` pattern earns at `member` (lowerCamel).
fn date_clause(member: &str, value: &str) -> String {
    format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {DATE_TIME_PATTERN}"
    )
}

/// The clause a `Filters` map whose keys are not all admissible earns.
fn filters_clause(rendered_map: &str) -> String {
    format!(
        "Value '{rendered_map}' at 'filters' failed to satisfy constraint: \
         {FILTER_KEY_REQUIREMENT}"
    )
}

/// The two operations, each with the ARN member it resolves a resource by, a well-formed
/// ARN of that kind the store holds nothing under, and one it holds a resource under.
struct Op {
    name: &'static str,
    arn_member: &'static str,
    absent_arn: String,
    stored_arn: String,
}

fn operations() -> [Op; 2] {
    [
        Op {
            name: "QueryForecast",
            arn_member: "ForecastArn",
            absent_arn: arn("forecast/never_created"),
            stored_arn: arn("forecast/electricity_demand"),
        },
        Op {
            name: "QueryWhatIfForecast",
            arn_member: "WhatIfForecastArn",
            absent_arn: arn("what-if-forecast/never_created/variant"),
            stored_arn: arn("what-if-forecast/electricity_analysis/price_cut"),
        },
    ]
}

/// Seed one forecast and one what-if forecast, as the control plane's create operations
/// would, under the ARNs [`operations`] reports as stored.
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

// ---------------------------------------------------------------------------
// Rule 1: the DateTime shape pattern
// ---------------------------------------------------------------------------

/// A `StartDate`/`EndDate` outside the pattern is `ValidationException` (400) naming the
/// member and quoting the value — and it is the answer whether the ARN names a stored
/// resource or nothing at all, because the shape layer runs before resolution.
///
/// The rows differ along the FORM of the value: a separator the layout does not use, a
/// missing field, an unpadded field, a reordered one, a non-date, and the empty string.
/// The accepted rows are the ISO 8601 spellings the pattern admits, each of which must
/// NOT be a shape refusal — whatever the service then makes of it.
#[tokio::test]
async fn a_date_outside_the_shape_pattern_is_refused_before_the_arn_is_resolved() {
    let (engine, router) = harness();
    seed(&engine);

    let rejected: [&str; 9] = [
        "2024/01/01 00:00:00", // slashes and a space: not the layout at all
        "2024-01-01",          // a date with no time of day
        "2024-01-01T00",       // an hour with no minutes
        "2024-1-1T00:00:00",   // unpadded month and day
        "01-01-2024T00:00:00", // day first
        "2024-01-01 00:00:00", // a space where the layout has 'T'
        "1704067200",          // epoch seconds
        "tomorrow",
        "",
    ];
    // Spellings the pattern admits: the documented layout, and the ISO 8601 forms that
    // extend it (no seconds, a zone designator, fractional seconds, an offset, a
    // lowercase date/time separator).
    let accepted: [&str; 6] = [
        "2024-01-01T00:00:00",
        "2024-01-01T00:00",
        "2024-01-01T00:00:00Z",
        "2024-01-01T00:00:00.000Z",
        "2024-01-01T00:00:00+01:00",
        "2024-01-01t00:00:00",
    ];

    for op in operations() {
        for (member, path) in [("StartDate", "startDate"), ("EndDate", "endDate")] {
            for resolution in ["absent", "stored"] {
                let queried = if resolution == "absent" {
                    &op.absent_arn
                } else {
                    &op.stored_arn
                };
                for value in rejected {
                    let case = (op.name, member, resolution, value);
                    let mut request = json!({
                        op.arn_member: queried,
                        "Filters": {"item_id": "client_21"},
                    });
                    request[member] = json!(value);
                    let (status, body) = call(&router, op.name, &request).await;
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status} with {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "ValidationException",
                        "case {case:?}: expected ValidationException, got {body}"
                    );
                    assert_eq!(
                        error_message(&body),
                        one_violation(&date_clause(path, value)),
                        "case {case:?}: the refusal must name the member and the value, \
                         got {body}"
                    );
                }
                for value in accepted {
                    let case = (op.name, member, resolution, value);
                    let mut request = json!({
                        op.arn_member: queried,
                        "Filters": {"item_id": "client_21"},
                    });
                    request[member] = json!(value);
                    let (status, body) = call(&router, op.name, &request).await;
                    assert_ne!(
                        error_code(&body),
                        "ValidationException",
                        "case {case:?}: the shape layer must admit this spelling, got \
                         {status} with {body}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the Filters key shape
// ---------------------------------------------------------------------------

/// A `Filters` KEY outside the key shape's pattern or length is `ValidationException`
/// (400) reporting the map as sent, before the ARN is resolved — while a key inside them
/// reaches resolution, which is what answers.
///
/// The rows differ along WHICH character takes a key outside the pattern, the key's size
/// against the `@length` bound, and the number of entries in the map.
#[tokio::test]
async fn a_filters_key_outside_the_key_shape_is_refused_before_the_arn_is_resolved() {
    let (engine, router) = harness();
    seed(&engine);

    // (case, the Filters map sent, the map as the message must render it)
    let rejected: Vec<(&str, Value, String)> = vec![
        (
            "a space and a bang",
            json!({"item id!": "client_21"}),
            "{item id!=client_21}".to_string(),
        ),
        (
            "a dot",
            json!({"item.id": "client_21"}),
            "{item.id=client_21}".to_string(),
        ),
        (
            "a slash",
            json!({"item/id": "client_21"}),
            "{item/id=client_21}".to_string(),
        ),
        (
            "a non-ASCII letter",
            json!({"itém_id": "client_21"}),
            "{itém_id=client_21}".to_string(),
        ),
        (
            "an empty key",
            json!({"": "client_21"}),
            "{=client_21}".to_string(),
        ),
        (
            // Two entries: one offending key is enough, and the map renders ordered by
            // key, which a map's own wire order cannot supply.
            "one good key beside one bad one",
            json!({"item_id": "client_21", "store front": "seattle"}),
            "{item_id=client_21, store front=seattle}".to_string(),
        ),
    ];

    for op in operations() {
        for (name, filters, rendered) in &rejected {
            // The ARN names nothing, so an answer other than the shape refusal would be
            // the 404 — which is exactly what this rule orders before itself.
            let case = (op.name, *name);
            let (status, body) = call(
                &router,
                op.name,
                &json!({op.arn_member: op.absent_arn, "Filters": filters}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                one_violation(&filters_clause(rendered)),
                "case {case:?}: the refusal must report the map and the key requirement, \
                 got {body}"
            );
        }

        // The `@length` maximum is a published bound, so it is tested AT the bound (a key
        // the shape admits, so resolution answers) and one character past it (refused).
        let at_bound = "k".repeat(256);
        let past_bound = "k".repeat(257);
        let (status, body) = call(
            &router,
            op.name,
            &json!({op.arn_member: op.absent_arn, "Filters": {at_bound.clone(): "client_21"}}),
        )
        .await;
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case ({:?}, \"a 256-character key\"): a key at the bound must reach \
             resolution, got {status} with {body}",
            op.name
        );
        let (status, body) = call(
            &router,
            op.name,
            &json!({op.arn_member: op.absent_arn, "Filters": {past_bound.clone(): "client_21"}}),
        )
        .await;
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case ({:?}, \"a 257-character key\"): expected ValidationException, got \
             {status} with {body}",
            op.name
        );
        let message = error_message(&body);
        assert!(
            message.ends_with(FILTER_KEY_REQUIREMENT),
            "case ({:?}, \"a 257-character key\"): expected the key requirement, got \
             {message:?}",
            op.name
        );
        // The map renders as `{key=value}`, which is past the length a message echoes, so
        // the echo must be a prefix plus the size of what was sent.
        let rendered = format!("{{{past_bound}=client_21}}");
        assert!(
            message.contains(&format!("... [{} bytes]", rendered.len())),
            "case ({:?}, \"a 257-character key\"): a map too long to echo must be elided, \
             got {message:?}",
            op.name
        );

        // Keys the shape admits, differing in character class and size: each reaches
        // resolution, which answers the 404 for this absent ARN.
        for key in ["item_id", "ITEM-ID9", "a", "0"] {
            let case = (op.name, key);
            let (status, body) = call(
                &router,
                op.name,
                &json!({op.arn_member: op.absent_arn, "Filters": {key: "client_21"}}),
            )
            .await;
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: a key the shape admits must reach resolution, got \
                 {status} with {body}"
            );
        }
    }
}

/// Several shape violations in one request are reported in ONE envelope, counted and
/// joined the way a shape refusal words them.
#[tokio::test]
async fn several_shape_violations_are_reported_in_one_envelope() {
    let (engine, router) = harness();
    seed(&engine);
    let [op, _] = operations();

    let (status, body) = call(
        &router,
        op.name,
        &json!({
            op.arn_member: op.stored_arn,
            "Filters": {"item_id": "client_21"},
            "StartDate": "2024/01/01 00:00:00",
            "EndDate": "yesterday",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"two bad dates\": expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_message(&body),
        format!(
            "2 validation errors detected: {}; {}",
            date_clause("startDate", "2024/01/01 00:00:00"),
            date_clause("endDate", "yesterday")
        ),
        "case \"two bad dates\": expected both clauses in one envelope, got {body}"
    );

    let (status, body) = call(
        &router,
        op.name,
        &json!({
            op.arn_member: op.stored_arn,
            "Filters": {"item id!": "client_21"},
            "StartDate": "tomorrow",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a bad date and a bad filter key\": expected HTTP 400, got {status} with \
         {body}"
    );
    assert_eq!(
        error_message(&body),
        format!(
            "2 validation errors detected: {}; {}",
            date_clause("startDate", "tomorrow"),
            filters_clause("{item id!=client_21}")
        ),
        "case \"a bad date and a bad filter key\": expected both clauses in one envelope, \
         got {body}"
    );
}
