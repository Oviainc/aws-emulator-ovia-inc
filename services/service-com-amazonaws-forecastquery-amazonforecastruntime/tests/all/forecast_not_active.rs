// SPDX-License-Identifier: CC0-1.0

//! Querying a Forecast resource whose creation run has not finished yet.
//!
//! The rule pinned here is one sentence for the whole class:
//!
//! > A query naming a resource of the kind the operation reads that the store HOLDS, but
//! > whose creation run has not reached the instant it becomes `ACTIVE`, is refused with
//! > the modelled `ResourceInUseException` (HTTP 409) and a message quoting the submitted
//! > ARN; the identical request is answered once that instant has passed.
//!
//! The condition comes from the control plane's own contract — "The Status of the forecast
//! must be ACTIVE before you can query or export the forecast" — so it is the ONE
//! documented conflict a pure data-plane read of this service has, which is why
//! `ResourceInUseException` (errorKind client, httpError 409) is modelled on both
//! `QueryForecast` and `QueryWhatIfForecast` and on nothing else here
//! (`smithy_query(query="operation",operation="QueryForecast,QueryWhatIfForecast",
//! model_path="services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json")`).
//! Both ARN-typed members of the service are therefore in the table below.
//!
//! Where the refusal sits is part of the rule, so the rows also pin its PRECEDENCE: it is
//! behind what judges the request itself (an ARN this service cannot read as one of the
//! queried kind, a `DateTime` outside its shape pattern — both 400) and behind resolution
//! (an ARN the store holds nothing under — 404), and ahead of every judgement that depends
//! on the resource's CONTENT (a continuation token, the filter keys, which series the
//! filters select, the date window) — because none of those can be answered from a
//! forecast that is not readable yet.
//!
//! It is also a READ: the rows repeat each refusal and then let the clock run past the due
//! instant, so a forecast that was refused still becomes queryable without any caller
//! having moved it, and the same query keeps answering afterwards.
//!
//! The rows differ along what the rule generalises over: which operation is called, how far
//! off the creation run's due instant is (and whether it has already passed), what else the
//! request carries, and what the store holds beside the queried resource.
//!
//! Every row drives the service the way a client does: an awsJson1_1 `POST /` through the
//! real router, so the boundary validator, the handler and the error envelope all run.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{
    ResourceKind, StoredForecast, StoredPoint, StoredSeries, FORECAST, WHAT_IF_FORECAST,
};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // which outcome a request earns, so it turns verification off rather than signing.
    router.set_request_authentication(false);
    router.register_service(forecastquery::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// An ARN of the shape the control plane mints.
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

/// A forecast with one series (`item_id=client_21`) and one statistic, so a resource that
/// is readable can be told from one that is not by more than its status.
fn stored_forecast(forecast_arn: &str) -> StoredForecast {
    let series = StoredSeries::new([("item_id", "client_21")]).with_statistic(
        "p50",
        [
            StoredPoint::new("2015-01-01T00:00:00", 42.0),
            StoredPoint::new("2015-01-02T00:00:00", 43.5),
        ],
    );
    StoredForecast::new(forecast_arn, "electricity_demand", "D", ["0.5"]).with_series(series)
}

/// One operation of this service: the resource kind its ARN member resolves, an ARN of
/// that kind, and the map of the store the control plane publishes such a resource into.
struct Op {
    /// The `X-Amz-Target` operation name.
    name: &'static str,
    /// The request member the ARN arrives in.
    member: &'static str,
    /// The resource kind that member must name — the table this service words an ARN
    /// refusal and a not-ready refusal from.
    kind: ResourceKind,
    /// The ARN resource field of a resource of that kind, and of another one of the same
    /// kind that is already readable.
    resource: &'static str,
    other_resource: &'static str,
    /// Publish `stored` as the control plane does for this operation's resource kind.
    put: fn(&InMemoryEngine, StoredForecast),
}

/// The base forecast a what-if forecast records as its provenance.
const BASE_FORECAST: &str = "forecast/electricity_demand";

fn operations() -> [Op; 2] {
    [
        Op {
            name: "QueryForecast",
            member: "ForecastArn",
            kind: FORECAST,
            resource: BASE_FORECAST,
            other_resource: "forecast/another_demand",
            put: |engine, stored| {
                engine
                    .state::<ForecastQueryState>()
                    .lock()
                    .put_forecast(stored);
            },
        },
        Op {
            name: "QueryWhatIfForecast",
            member: "WhatIfForecastArn",
            kind: WHAT_IF_FORECAST,
            resource: "what-if-forecast/electricity_analysis/price_cut",
            other_resource: "what-if-forecast/electricity_analysis/price_rise",
            put: |engine, stored| {
                engine
                    .state::<ForecastQueryState>()
                    .lock()
                    .put_what_if_forecast(stored.derived_from(arn(BASE_FORECAST)));
            },
        },
    ]
}

/// Publish the resource `op` queries, due to become readable at `active_at`, and answer
/// the ARN a query names it by.
fn publish(engine: &InMemoryEngine, op: &Op, active_at: SystemTime) -> String {
    let resource_arn = arn(op.resource);
    (op.put)(engine, stored_forecast(&resource_arn).active_from(active_at));
    resource_arn
}

/// How far off a creation run's due instant can be while the resource is not readable.
/// Each is a span long enough that it cannot elapse while a row runs, so the row tests the
/// rule and not the test's own timing; the transition itself is timed by
/// [`a_forecast_becomes_queryable_when_its_creation_run_falls_due`].
fn pending_spans() -> Vec<(&'static str, Duration)> {
    vec![
        ("due in ten seconds", Duration::from_secs(10)),
        ("due in a minute", Duration::from_secs(60)),
        ("due in an hour", Duration::from_secs(60 * 60)),
        ("due in a day", Duration::from_secs(24 * 60 * 60)),
    ]
}

/// Everything else a well-formed request can carry at the same time. None of it changes the
/// answer: each one is a question about the resource's CONTENT, which a resource that is not
/// readable yet has no answer for.
fn also_sent() -> Vec<(&'static str, Value)> {
    vec![
        ("nothing else", json!({})),
        (
            "plus a date window",
            json!({"StartDate": "2015-01-01T00:00:00", "EndDate": "2015-01-06T00:00:00"}),
        ),
        (
            "plus a date window that selects no point",
            json!({"StartDate": "2030-01-01T00:00:00", "EndDate": "2030-01-06T00:00:00"}),
        ),
        (
            "plus a continuation token this service never minted",
            json!({"NextToken": "bogus-next-token"}),
        ),
        (
            "with filters that match no stored series",
            json!({"Filters": {"item_id": "client_99"}}),
        ),
        (
            "with filters naming a dimension this forecast does not carry",
            json!({"Filters": {"region": "eu_central"}}),
        ),
        (
            "with two filter criteria",
            json!({"Filters": {"item_id": "client_21", "store": "store_7"}}),
        ),
    ]
}

/// Fills the store with whatever a row holds BESIDE the queried resource.
type Seeder = fn(&InMemoryEngine, &Op);

/// What else the store holds when the not-ready resource is queried. None of it matters: the
/// answer is about the resource the ARN names.
fn store_states() -> Vec<(&'static str, Seeder)> {
    vec![
        ("nothing else stored", |_engine, _op| {}),
        (
            "beside a readable resource of the same kind",
            |engine, op| {
                let other = arn(op.other_resource);
                (op.put)(engine, stored_forecast(&other));
            },
        ),
        (
            "beside the SAME ARN stored as the other resource kind, readable",
            |engine, op| {
                // The two maps are separate, so a readable resource of the OTHER kind
                // under the very same ARN cannot make this one readable.
                let queried = arn(op.resource);
                let service_state = engine.state::<ForecastQueryState>();
                let mut state = service_state.lock();
                if op.name == "QueryForecast" {
                    state.put_what_if_forecast(stored_forecast(&queried));
                } else {
                    state.put_forecast(stored_forecast(&queried));
                }
            },
        ),
    ]
}

/// Merge `extra`'s members into `body`, so a row states only what it varies.
fn with_extra(body: &mut Value, extra: &Value) {
    let object = body.as_object_mut().expect("request body is an object");
    for (key, value) in extra.as_object().expect("extra members are an object") {
        object.insert(key.clone(), value.clone());
    }
}

/// The whole class, over both operations: a resource the store HOLDS whose creation run is
/// not due yet earns HTTP 409 `ResourceInUseException`, worded by the one rule both
/// operations share and quoting the ARN as submitted — whatever else the request carries and
/// whatever else the store holds.
#[tokio::test]
async fn a_resource_whose_creation_run_is_not_due_yet_is_refused_as_in_use() {
    for op in operations() {
        for (span_case, span) in pending_spans() {
            for (also, extra) in also_sent() {
                for (store_case, seed) in store_states() {
                    // A fresh store per row: what else the store held is one of the things
                    // this rule must be independent of.
                    let (engine, router) = harness();
                    let queried = publish(&engine, &op, SystemTime::now() + span);
                    seed(&engine, &op);

                    let case = format!("{} / {span_case} / {also} / {store_case}", op.name);
                    let mut body =
                        json!({op.member: &queried, "Filters": {"item_id": "client_21"}});
                    with_extra(&mut body, &extra);

                    let (status, response) = call(&router, op.name, &body).await;
                    assert_eq!(
                        status, 409,
                        "case {case:?}: expected HTTP 409, got {status} with {response}"
                    );
                    assert_eq!(
                        error_code(&response),
                        "ResourceInUseException",
                        "case {case:?}: expected ResourceInUseException, got {response}"
                    );
                    let actual = error_message(&response);
                    assert!(
                        actual.contains(&queried),
                        "case {case:?}: expected the message to quote the submitted ARN \
                         {queried:?}, got {actual:?}"
                    );
                    assert_eq!(
                        actual,
                        op.kind.not_active_message(&queried),
                        "case {case:?}: expected the wording both operations share for a \
                         resource that is not ACTIVE yet, got {actual:?}"
                    );
                }
            }
        }
    }
}

/// The refusal is a READ, and the clock is what lifts it: a resource refused as in use
/// becomes queryable once its creation run falls due, without any caller having moved it,
/// and repeating either call changes nothing.
#[tokio::test]
async fn a_forecast_becomes_queryable_when_its_creation_run_falls_due() {
    // Long enough that the refusals below cannot race the transition, short enough to wait
    // out: the span a creation run takes is the control plane's, not this suite's.
    const PENDING: Duration = Duration::from_millis(300);

    for op in operations() {
        let (engine, router) = harness();
        let queried = publish(&engine, &op, SystemTime::now() + PENDING);
        let body = json!({op.member: &queried, "Filters": {"item_id": "client_21"}});

        // Repeated while it is not due: a read neither moves the resource on nor consumes
        // anything, so the second and third answers are the first.
        for attempt in 1..=3 {
            let case = format!("{} / refused attempt {attempt}", op.name);
            let (status, response) = call(&router, op.name, &body).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected HTTP 409, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceInUseException",
                "case {case:?}: expected ResourceInUseException, got {response}"
            );
        }

        ::tokio::time::sleep(PENDING + Duration::from_millis(200)).await;

        // Due: the identical request is answered from the stored series, and keeps being.
        for attempt in 1..=2 {
            let case = format!("{} / answered attempt {attempt}", op.name);
            let (status, response) = call(&router, op.name, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 once the creation run is due, got \
                 {status} with {response}"
            );
            let points = response
                .pointer("/Forecast/Predictions/p50")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                points.len(),
                2,
                "case {case:?}: expected the two stored points of the series, got \
                 {response}"
            );
        }
    }
}

/// A resource the control plane published as readable from the start is readable from the
/// start: the refusal is owed to a creation run that is not due, not to every stored
/// resource.
#[tokio::test]
async fn a_resource_already_due_is_answered() {
    for op in operations() {
        for (case, active_at) in [
            ("published as already ACTIVE", SystemTime::now()),
            (
                "a run that fell due a minute ago",
                SystemTime::now() - Duration::from_secs(60),
            ),
        ] {
            let (engine, router) = harness();
            let queried = publish(&engine, &op, active_at);
            let case = format!("{} / {case}", op.name);

            let (status, response) = call(
                &router,
                op.name,
                &json!({op.member: &queried, "Filters": {"item_id": "client_21"}}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {response}"
            );
            assert!(
                response
                    .pointer("/Forecast/Predictions/p50")
                    .and_then(Value::as_array)
                    .is_some_and(|points| points.len() == 2),
                "case {case:?}: expected the stored series, got {response}"
            );
        }
    }
}

/// The refusal's PRECEDENCE, so the rule cannot swallow its neighbours: what judges the
/// request itself and what resolves the resource both come first, and what depends on the
/// resource's content comes after.
#[tokio::test]
async fn the_in_use_refusal_sits_between_resolution_and_the_resources_content() {
    for op in operations() {
        let (engine, router) = harness();
        let queried = publish(&engine, &op, SystemTime::now() + Duration::from_secs(60));

        // Ahead of it: the ARN member itself. A value this service cannot read as one of
        // the queried kind names no resource at all, so there is no status to report.
        for (case, sent) in [
            ("a value that is not an ARN", "definitely-not-an-arn".to_string()),
            (
                "an ARN naming the other Forecast resource kind",
                if op.name == "QueryForecast" {
                    arn("what-if-forecast/electricity_analysis/price_cut")
                } else {
                    arn(BASE_FORECAST)
                },
            ),
        ] {
            let case = format!("{} / {case}", op.name);
            let (status, response) = call(
                &router,
                op.name,
                &json!({op.member: &sent, "Filters": {"item_id": "client_21"}}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {response}"
            );
        }

        // Ahead of it: the SHAPE of the rest of the request, judged before the resource is
        // looked at, so a date this service cannot read is still a 400.
        let (status, response) = call(
            &router,
            op.name,
            &json!({
                op.member: &queried,
                "Filters": {"item_id": "client_21"},
                "StartDate": "2015/01/01 00:00:00",
            }),
        )
        .await;
        let case = format!("{} / a date outside its shape pattern", op.name);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );

        // Ahead of it: resolution. An ARN of the queried kind the store holds nothing under
        // is absent, not in use — a client must not wait for a resource that is not there.
        let absent = if op.name == "QueryForecast" {
            arn("forecast/never_created")
        } else {
            arn("what-if-forecast/electricity_analysis/never_created")
        };
        let (status, response) = call(
            &router,
            op.name,
            &json!({op.member: &absent, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        let case = format!("{} / an ARN the store holds nothing under", op.name);
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );

        // After it: what depends on the resource's content. Once the resource IS readable,
        // the very requests that earned the 409 above are answered on their own terms.
        let (engine, router) = harness();
        let queried = publish(&engine, &op, SystemTime::now());
        for (case, extra, status_wanted, code_wanted) in [
            (
                "a continuation token this service never minted",
                json!({"NextToken": "bogus-next-token"}),
                400,
                "InvalidNextTokenException",
            ),
            (
                "filters that match no stored series",
                json!({"Filters": {"item_id": "client_99"}}),
                404,
                "ResourceNotFoundException",
            ),
            (
                "filters naming a dimension this forecast does not carry",
                json!({"Filters": {"region": "eu_central"}}),
                400,
                "InvalidInputException",
            ),
        ] {
            let case = format!("{} / readable, {case}", op.name);
            let mut body = json!({op.member: &queried, "Filters": {"item_id": "client_21"}});
            with_extra(&mut body, &extra);
            let (status, response) = call(&router, op.name, &body).await;
            assert_eq!(
                status, status_wanted,
                "case {case:?}: expected HTTP {status_wanted}, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                code_wanted,
                "case {case:?}: expected {code_wanted}, got {response}"
            );
        }
    }
}
