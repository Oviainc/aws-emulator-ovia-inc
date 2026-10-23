// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a resource ARN it can read but holds no resource for.
//!
//! The rule pinned here is one sentence for the whole class:
//!
//! > A request whose resource-ARN member names a resource of the kind the operation
//! > queries, but that this service holds nothing under, is refused with the modelled
//! > `ResourceNotFoundException` (HTTP 404) and the message `No resource found ` followed
//! > by the ARN exactly as the request supplied it — no other prose, whatever else the
//! > request carries and whatever else the store holds.
//!
//! So the message is REQUEST-DERIVED: a caller can parse the offending ARN back out of
//! it, which is why nothing here accepts the shape's documentation sentence or any
//! emulator-authored wording around the value. The two ARN members
//! (`QueryForecast$ForecastArn`, target `Arn` length 0..256, and
//! `QueryWhatIfForecast$WhatIfForecastArn`, target `LongArn` length 0..300, both with
//! pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$`) are the only ARN-typed members the model
//! declares — `smithy_query(query="closure",operation="QueryForecast,QueryWhatIfForecast",
//! model_path="services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json")`
//! — so the table below spans both of them.
//!
//! The rows differ along what the rule generalises over: which operation is called, the
//! ARN's partition, region and account, the spelling and the LENGTH of the resource name
//! (including one exactly at the member's own modelled maximum), what ELSE the request
//! carries (dates, a continuation token, extra filter criteria), and what the store holds
//! at the time (nothing, another resource of the same kind, the same name under the OTHER
//! kind). Two negative controls keep the rule from swallowing its neighbours: an ARN that
//! cannot be read as one of the queried kind is still the `InvalidInputException` (400)
//! worded `Invalid ARN for <type>: <value>` (`invalid_arn_message.rs`), and an ARN that
//! names a stored resource is still answered.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

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

/// A forecast with one series (`item_id=client_21`) and one statistic, so a resolvable
/// ARN can be told from an absent one by more than its error.
fn stored_forecast(forecast_arn: &str) -> StoredForecast {
    let series = StoredSeries::new([("item_id", "client_21")]).with_statistic(
        "p50",
        [StoredPoint::new("2015-01-01T00:00:00", 42.0)],
    );
    StoredForecast::new(forecast_arn, "electricity_demand", "D", ["0.5"]).with_series(series)
}

/// One operation of this service, and what its ARN member resolves.
struct Op {
    /// The `X-Amz-Target` operation name.
    name: &'static str,
    /// The request member the ARN arrives in.
    member: &'static str,
    /// The ARN resource field of a resource of the kind this operation queries, with
    /// `{name}` standing for the (last) resource name: a forecast carries one name, a
    /// what-if forecast the analysis and the forecast (`crate::state::ResourceKind`).
    resource: fn(&str) -> String,
    /// The modelled maximum length of that member: `Arn` 256, `LongArn` 300
    /// (`smithy_query(query="closure", …)`).
    max_length: usize,
}

fn operations() -> [Op; 2] {
    [
        Op {
            name: "QueryForecast",
            member: "ForecastArn",
            resource: |name| format!("forecast/{name}"),
            max_length: 256,
        },
        Op {
            name: "QueryWhatIfForecast",
            member: "WhatIfForecastArn",
            resource: |name| format!("what-if-forecast/electricity_analysis/{name}"),
            max_length: 300,
        },
    ]
}

/// The ARN fields a Forecast ARN carries besides its resource: `(case, partition, region,
/// account)`. None of them is judged by either operation — they are values the control
/// plane chose — so each one is simply a resource this emulator does not hold.
fn arn_scopes() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        // The recorded instance's scope.
        ("the commercial partition", "aws", "us-west-2", "000000000000"),
        ("another region", "aws", "eu-central-1", "123456789012"),
        ("the China partition", "aws-cn", "cn-north-1", "210987654321"),
        ("the GovCloud partition", "aws-us-gov", "us-gov-west-1", "000000000001"),
        // The modelled pattern leaves region and account free (`.*:.*`).
        ("an empty region and account", "aws", "", ""),
    ]
}

/// Resource names an absent resource can be asked for under: each a different spelling the
/// modelled pattern (`…:.+$`) admits.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        // The recorded instance's name shape.
        ("a snake-cased name", "sim2real_fq_288054bad79e".to_string()),
        ("a hyphenated name", "does-not-exist-abc".to_string()),
        ("a dotted name", "electricity.demand.v2".to_string()),
        ("a single character", "x".to_string()),
        ("a digit-only name", "20240101".to_string()),
        ("a name with a colon in it", "tenant:42".to_string()),
    ]
}

/// Everything else the request can carry at the same time. Resource resolution comes
/// after the ARN is read and before a token is judged, so none of these changes the
/// answer.
fn also_sent() -> Vec<(&'static str, Value)> {
    vec![
        ("nothing else", json!({})),
        (
            "plus a date window",
            json!({"StartDate": "2015-01-01T00:00:00", "EndDate": "2015-01-06T00:00:00"}),
        ),
        (
            "plus a continuation token this service never minted",
            json!({"NextToken": "bogus-next-token"}),
        ),
        (
            "plus a second filter criterion",
            json!({"Filters": {"item_id": "client_21", "store": "store_7"}}),
        ),
    ]
}

/// Fills the store before a row's call, given the ARN that row queries.
type Seeder = fn(&InMemoryEngine, &str);

/// What the store holds when the absent ARN is queried: `(case, seeder)`. An absent
/// resource is absent whether the store is empty, holds another resource of the same kind,
/// or holds the SAME name under the other kind — the two maps are separate.
fn store_states() -> Vec<(&'static str, Seeder)> {
    vec![
        ("an empty store", |_engine, _arn| {}),
        ("a store holding another forecast", |engine, _arn| {
            let other = engine::emulator_arn("forecast", "us-east-1", "forecast/something_else");
            engine
                .state::<ForecastQueryState>()
                .lock()
                .put_forecast(stored_forecast(&other));
        }),
        (
            "a store holding this very ARN under the OTHER resource kind",
            |engine, arn| {
                // `QueryForecast` reads `forecasts` and `QueryWhatIfForecast` reads
                // `what_if_forecasts`; seeding the queried ARN into the map the operation
                // does NOT read must leave the resource absent.
                let service_state = engine.state::<ForecastQueryState>();
                let mut state = service_state.lock();
                if arn.contains(":what-if-forecast/") {
                    state.put_forecast(stored_forecast(arn));
                } else {
                    state.put_what_if_forecast(stored_forecast(arn));
                }
            },
        ),
    ]
}

/// The whole class, over both operations: an ARN of the queried kind that the store holds
/// nothing under earns HTTP 404, `ResourceNotFoundException`, and `No resource found `
/// plus the ARN as submitted — nothing more.
#[tokio::test]
async fn an_absent_resource_is_reported_as_no_resource_found_plus_the_submitted_arn() {
    for op in operations() {
        let Op {
            name,
            member,
            resource,
            max_length,
        } = op;

        // (case, the ARN sent)
        let mut sent_arns: Vec<(String, String)> = Vec::new();
        for (scope_case, partition, region, account) in arn_scopes() {
            for (name_case, resource_name) in absent_names() {
                sent_arns.push((
                    format!("{scope_case} / {name_case}"),
                    format!(
                        "arn:{partition}:forecast:{region}:{account}:{}",
                        resource(&resource_name)
                    ),
                ));
            }
        }
        // A bound, so it is tested AT the member's modelled maximum: an ARN exactly that
        // long is readable, so it resolves (and is absent) rather than being refused —
        // one past it is `invalid_arn_message.rs`'s row. Padded out from this operation's
        // own resource prefix so the row tests the LENGTH and nothing else.
        let padded = format!(
            "arn:aws:forecast:us-west-2:000000000000:{}",
            resource(&"n".repeat(max_length))
        );
        sent_arns.push((
            "exactly at the member's maximum length".to_string(),
            padded[..max_length].to_string(),
        ));

        for (arn_case, sent) in sent_arns {
            for (also, extra) in also_sent() {
                for (store_case, seed) in store_states() {
                    // A fresh store per row: what the store held before the call is one of
                    // the things this rule must be independent of.
                    let (engine, router) = harness();
                    seed(&engine, &sent);

                    let case = format!("{name} / {arn_case} / {also} / {store_case}");
                    let mut body = json!({member: &sent, "Filters": {"item_id": "client_21"}});
                    let object = body.as_object_mut().expect("request body is an object");
                    for (key, value) in extra.as_object().expect("extra members are an object") {
                        object.insert(key.clone(), value.clone());
                    }

                    let (status, response) = call(&router, name, &body).await;
                    assert_eq!(
                        status, 404,
                        "case {case:?}: expected HTTP 404, got {status} with {response}"
                    );
                    assert_eq!(
                        error_code(&response),
                        "ResourceNotFoundException",
                        "case {case:?}: expected ResourceNotFoundException, got {response}"
                    );
                    let actual = message(&response);
                    assert_eq!(
                        actual,
                        format!("No resource found {sent}"),
                        "case {case:?}: expected the message to be \"No resource found \" plus \
                         the submitted ARN and nothing else, got {actual:?}"
                    );
                }
            }
        }
    }
}

/// The neighbours of the rule, so it cannot swallow them: an ARN that cannot be read as
/// one of the queried kind is the 400 worded `Invalid ARN for <type>: <value>`, and an ARN
/// that names a STORED resource is answered with that resource's series.
#[tokio::test]
async fn the_not_found_wording_is_reserved_for_a_readable_arn_that_resolves_to_nothing() {
    let (engine, router) = harness();
    let forecast_arn = engine::emulator_arn("forecast", "us-east-1", "forecast/electricity_demand");
    let what_if_arn = engine::emulator_arn(
        "forecast",
        "us-east-1",
        "what-if-forecast/electricity_analysis/price_cut",
    );
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_forecast(stored_forecast(&forecast_arn));
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_what_if_forecast(stored_forecast(&what_if_arn).derived_from(&forecast_arn));

    // An unreadable value keeps the ARN-rejection wording, so a caller can still tell a
    // value it must fix from a resource that is not there.
    for (name, member, resource_type, sent) in [
        ("QueryForecast", "ForecastArn", "forecast", "definitely-not-an-arn"),
        (
            "QueryForecast",
            "ForecastArn",
            "forecast",
            // Readable as an ARN, but names a what-if forecast rather than a forecast.
            "arn:aws:forecast:us-east-1:000000000000:what-if-forecast/a/b",
        ),
        (
            "QueryWhatIfForecast",
            "WhatIfForecastArn",
            "what-if-forecast",
            "arn:aws:forecast:us-east-1:000000000000:forecast/electricity_demand",
        ),
    ] {
        let (status, response) = call(
            &router,
            name,
            &json!({member: sent, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        let case = format!("{name} / unreadable as a {resource_type} ARN / {sent}");
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            message(&response),
            format!("Invalid ARN for {resource_type}: {sent}"),
            "case {case:?}: expected the ARN-rejection wording, got {response}"
        );
    }

    // A stored resource is answered, not reported absent.
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
            "case {name:?} / a stored resource: expected HTTP 200, got {status} with {response}"
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
