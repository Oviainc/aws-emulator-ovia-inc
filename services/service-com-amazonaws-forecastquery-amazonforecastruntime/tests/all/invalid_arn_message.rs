// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS an ARN it cannot resolve to a resource of the kind the operation
//! queries.
//!
//! The rule pinned here is one sentence for the whole class:
//!
//! > A request whose resource-ARN member cannot be read as an ARN naming the resource kind
//! > that operation queries — whether it falls outside the member's modelled `@pattern` or
//! > `@length`, or satisfies both yet names another kind of resource — is refused with the
//! > modelled `InvalidInputException` (HTTP 400) and the message
//! > `Invalid ARN for <resource type>: <the value supplied>`, before any other member of
//! > the request is judged.
//!
//! So the answer does NOT distinguish the ways a value fails: Amazon Forecast quotes the
//! value back after the kind of resource that was expected and says nothing about the
//! ARN's internal form, which is why nothing here asserts a clause naming a constraint.
//! The two ARN members (`QueryForecast$ForecastArn`, target `Arn` length 0..256, and
//! `QueryWhatIfForecast$WhatIfForecastArn`, target `LongArn` length 0..300, both with
//! pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$`) are the only ARN-typed members the model
//! declares — `smithy_query(query="closure",operation="QueryForecast,QueryWhatIfForecast",
//! model_path="services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json")`
//! — so the table below spans both of them.
//!
//! The rows differ along what the rule generalises over: which operation is called, the
//! FORM of the bad value (not an ARN at all, empty, another service's ARN, a partition
//! outside the character class, past the member's own maximum length), the resource KIND
//! the ARN names instead, the number of name SEGMENTS it carries, and what ELSE is wrong
//! with the request at the same time. Two negative controls keep the rule from swallowing
//! its neighbours: an ARN that DOES name a resource of the queried kind and happens to
//! name nothing stored is still `ResourceNotFoundException` (404), and one that names a
//! stored resource is still answered.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// How many bytes of an echoed value survive into a message the validation layer in front
/// of the service produces: it elides a value a caller controls at `MAX_ECHOED_VALUE`
/// (`vendor/smithy-schema-server/src/constraints.rs`) so a request cannot be amplified
/// into an arbitrarily long response, rendering a longer one as
/// `<first MAX_ECHOED_VALUE bytes>... [<len> bytes]`. A value at or under it is quoted back
/// whole, which is every row here except the one that tests a length bound.
const MAX_ECHOED_VALUE: usize = 256;

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // how a rejection is worded, so it turns verification off rather than signing each
    // request.
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

/// The `Message` of a failure response, whatever casing the envelope used.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert the response is the modelled ARN rejection for `sent`: HTTP 400,
/// `InvalidInputException`, and `Invalid ARN for <resource_type>: <sent>`.
fn assert_invalid_arn(case: &str, status: u16, body: &Value, resource_type: &str, sent: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body}"
    );
    let actual = message(body);
    let prefix = format!("Invalid ARN for {resource_type}: ");
    let echoed = actual.strip_prefix(&prefix);
    assert!(
        echoed.is_some(),
        "case {case:?}: expected a message starting {prefix:?}, got {actual:?}"
    );
    let echoed = echoed.unwrap_or_default();
    if sent.len() <= MAX_ECHOED_VALUE {
        assert_eq!(
            echoed, sent,
            "case {case:?}: expected the message to quote back {sent:?}, got {echoed:?}"
        );
    } else {
        // Over the echo bound, so the value is quoted back as its leading bytes plus the
        // validation layer's own elision marker.
        let expected = format!("{}... [{} bytes]", &sent[..MAX_ECHOED_VALUE], sent.len());
        assert_eq!(
            echoed, expected,
            "case {case:?}: expected the message to quote back {expected:?}, got {echoed:?}"
        );
    }
}

/// One operation of this service, and what its ARN member resolves.
struct Op {
    /// The `X-Amz-Target` operation name.
    name: &'static str,
    /// The request member the ARN arrives in.
    member: &'static str,
    /// The resource type the rejection message names.
    resource_type: &'static str,
    /// The modelled maximum length of that member (`Arn` 256, `LongArn` 300).
    max_length: usize,
    /// ARNs that satisfy the member's constraints yet name no resource of the kind this
    /// operation queries: `(case, value)`.
    wrong_kind: Vec<(&'static str, String)>,
    /// An ARN that DOES name a resource of the queried kind, with nothing stored under it.
    absent: String,
}

fn operations() -> [Op; 2] {
    [
        Op {
            name: "QueryForecast",
            member: "ForecastArn",
            resource_type: "forecast",
            max_length: 256,
            wrong_kind: vec![
                (
                    "a what-if forecast ARN",
                    arn("what-if-forecast/electricity_analysis/price_cut"),
                ),
                ("a predictor ARN", arn("predictor/electricity_predictor")),
                (
                    "a forecast-export-job ARN: the type matches exactly, never by prefix",
                    arn("forecast-export-job/electricity_demand"),
                ),
                ("a forecast ARN with no name", arn("forecast/")),
                (
                    "a forecast ARN with a name segment too many",
                    arn("forecast/electricity_demand/extra"),
                ),
                ("a resource field with no type separator", arn("electricity_demand")),
            ],
            absent: arn("forecast/never_created"),
        },
        Op {
            name: "QueryWhatIfForecast",
            member: "WhatIfForecastArn",
            resource_type: "what-if-forecast",
            max_length: 300,
            wrong_kind: vec![
                (
                    // The recorded instance: a what-if forecast ARN carrying one name
                    // where its kind carries the analysis and the forecast.
                    "a what-if forecast ARN missing its analysis name",
                    arn("what-if-forecast/sim2real_fq_24f99d021b53"),
                ),
                (
                    "a what-if forecast ARN with a name segment too many",
                    arn("what-if-forecast/electricity_analysis/price_cut/extra"),
                ),
                ("a plain forecast ARN", arn("forecast/electricity_demand")),
                (
                    "a what-if analysis ARN",
                    arn("what-if-analysis/electricity_analysis"),
                ),
                ("a dataset ARN", arn("dataset/electricity_usage")),
                ("a resource field with no type separator", arn("electricity_demand")),
            ],
            absent: arn("what-if-forecast/never_created/variant"),
        },
    ]
}

/// Values no ARN member of this service can read, whatever the operation: each one a
/// different way of being outside the shared pattern.
fn malformed_values() -> Vec<(&'static str, String)> {
    vec![
        // The recorded instance.
        ("not an ARN at all", "definitely-not-an-arn".to_string()),
        ("the empty value", String::new()),
        (
            "another service's ARN",
            "arn:aws:s3:us-east-1:000000000000:bucket/x".to_string(),
        ),
        (
            "an empty resource field",
            "arn:aws:forecast:us-east-1:000000000000:".to_string(),
        ),
        (
            "a partition outside the character class",
            "arn:AWS:forecast:us-east-1:000000000000:forecast/x".to_string(),
        ),
        (
            "the ARN prefix missing",
            "aws:forecast:us-east-1:000000000000:forecast/x".to_string(),
        ),
        (
            "a URL rather than an ARN",
            "https://forecast.us-east-1.amazonaws.com/forecast/x".to_string(),
        ),
    ]
}

/// Everything else that can be wrong with the request at the same time, from the
/// operation's own input: a malformed date, a reversed range, a filter key outside its
/// pattern, a continuation token this service never minted — and all of them at once.
/// The ARN is read first, so none of these changes the answer.
fn also_wrong() -> Vec<(&'static str, Value)> {
    vec![
        ("nothing else wrong", json!({})),
        (
            "plus a malformed EndDate",
            json!({"EndDate": "01-01-2024 00:00:00"}),
        ),
        (
            "plus a reversed date range",
            json!({"StartDate": "2015-01-06T00:00:00", "EndDate": "2015-01-01T00:00:00"}),
        ),
        (
            "plus a bogus NextToken",
            json!({"NextToken": "bogus-next-token"}),
        ),
        (
            "plus a Filters key outside its pattern",
            json!({"Filters": {"item id!": "client_21"}}),
        ),
        (
            "plus every one of them",
            json!({
                "StartDate": "2015-01-06T00:00:00",
                "EndDate": "01-01-2024 00:00:00",
                "NextToken": "bogus-next-token",
                "Filters": {"item id!": "client_21"},
            }),
        ),
    ]
}

/// The whole class, over both operations: every unreadable ARN earns the same code, the
/// same status and the same ARN-derived message, whatever else the request carries.
#[tokio::test]
async fn an_arn_that_names_no_resource_of_the_queried_kind_is_quoted_back() {
    let (engine, router) = harness();
    for op in operations() {
        let Op {
            name,
            member,
            resource_type,
            max_length,
            wrong_kind,
            absent,
        } = op;

        // Every bad value this operation can be called with: the shared malformed forms,
        // the kinds it cannot resolve, and one past its own maximum length.
        let mut values: Vec<(String, String)> = malformed_values()
            .into_iter()
            .chain(wrong_kind)
            .map(|(case, value)| (case.to_string(), value))
            .collect();
        // A bound, so it is tested one past it. Padded out from this operation's own
        // resource prefix so the row tests the LENGTH and nothing else; the value at the
        // bound belongs to the negative controls below (it resolves, and is absent).
        let padded = arn(&format!("{resource_type}/{}", "x".repeat(max_length + 8)));
        values.push((
            "one character past the member's maximum length".to_string(),
            padded[..=max_length].to_string(),
        ));

        for (case, sent) in values {
            for (also, extra) in also_wrong() {
                let case = format!("{name} / {case} / {also}");
                let mut body = json!({member: sent, "Filters": {"item_id": "client_21"}});
                let object = body.as_object_mut().expect("request body is an object");
                for (key, value) in extra.as_object().expect("extra members are an object") {
                    object.insert(key.clone(), value.clone());
                }
                let (status, response) = call(&router, name, &body).await;
                assert_invalid_arn(&case, status, &response, resource_type, &sent);
            }
        }

        // Negative control: an ARN of the kind this operation DOES query, with nothing
        // stored under it, is a resource that is absent — not a value that cannot be
        // read.
        let (status, response) = call(
            &router,
            name,
            &json!({member: absent, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?} / an ARN of the queried kind naming nothing stored: expected \
             HTTP 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {name:?} / an ARN of the queried kind naming nothing stored: expected \
             ResourceNotFoundException, got {response}"
        );
        assert!(
            !message(&response).starts_with("Invalid ARN for"),
            "case {name:?} / an ARN of the queried kind naming nothing stored: an absent \
             resource must not be reported as an unreadable ARN, got {response}"
        );
    }

    // Negative control: the same members, carrying ARNs of the queried kind that the
    // store holds, are still answered — the rule above refuses values, not requests.
    let forecast_arn = arn("forecast/electricity_demand");
    let what_if_arn = arn("what-if-forecast/electricity_analysis/price_cut");
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_forecast(stored_forecast(&forecast_arn));
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_what_if_forecast(stored_forecast(&what_if_arn).derived_from(&forecast_arn));

    for (name, member, stored) in [
        ("QueryForecast", "ForecastArn", &forecast_arn),
        ("QueryWhatIfForecast", "WhatIfForecastArn", &what_if_arn),
    ] {
        let (status, response) = call(
            &router,
            name,
            &json!({member: stored, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?} / a stored resource: expected HTTP 200, got {status} with \
             {response}"
        );
        assert!(
            response
                .pointer("/Forecast/Predictions/p50")
                .and_then(Value::as_array)
                .is_some_and(|points| points.len() == 1),
            "case {name:?} / a stored resource: expected the stored series, got {response}"
        );
    }
}
