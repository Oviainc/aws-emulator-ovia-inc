// SPDX-License-Identifier: CC0-1.0

//! The ONE post-resolution decision both query operations render.
//!
//! The rule pinned here is one sentence for the whole class:
//!
//! > Once a query has looked its ARN up in this service's store, WHICH condition is
//! > judged, in which order, and with which message is a single decision — the
//! > [`QueryOutcome`] `ResourceKind::resolve_query` computes from the resource kind, the
//! > stored resource and the request — and each operation only renders that outcome into
//! > its own modelled errors, so both answer the same status, the same wire code and the
//! > same message rule for every request.
//!
//! So each row below drives ONE request through the real router and compares it against
//! the outcome the shared decision computes for that request, rather than against a
//! status written out by hand; and every row is run through BOTH operations, whose
//! statuses and codes must agree. A handler that re-implemented one of the branches —
//! judging a continuation token ahead of the readiness gate, say, or classifying a
//! rejection as 400 where the shared decision classifies it 404 — fails here even if the
//! other handler still agrees with the rule.
//!
//! The rows differ along what the decision generalises over: whether the store holds the
//! queried resource, whether its creation run is due, whether a continuation token was
//! sent, which filters select the series (none, one, several), and which date window
//! clips it.
//!
//! Every row drives the service the way a client does: an awsJson1_1 `POST /` through the
//! real router, so the boundary validator, the handler and the error envelope all run.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{
    QueryOutcome, QueryRequest, ResourceKind, StoredForecast, StoredPoint, StoredSeries, FORECAST,
    WHAT_IF_FORECAST,
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

/// `Forecast.Predictions` as `statistic -> [(timestamp, value)]`.
fn response_predictions(body: &Value) -> BTreeMap<String, Vec<(String, f64)>> {
    body.pointer("/Forecast/Predictions")
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

/// The shared decision's own projection, in the same rendering, so a 200 is compared
/// against what the decision said rather than against a transcript.
fn outcome_predictions(
    points: &[(String, Vec<StoredPoint>)],
) -> BTreeMap<String, Vec<(String, f64)>> {
    points
        .iter()
        .map(|(statistic, points)| {
            (
                statistic.clone(),
                points
                    .iter()
                    .map(|point| (point.timestamp.clone(), point.value))
                    .collect(),
            )
        })
        .collect()
}

/// The HTTP status and wire code every [`QueryOutcome`] is rendered as, by either
/// operation: the statuses the model declares for the errors each variant names
/// (`smithy_query(query="operation",operation="QueryForecast,QueryWhatIfForecast")`).
fn rendering(outcome: &QueryOutcome) -> (u16, &'static str) {
    match outcome {
        QueryOutcome::Predictions(_) => (200, ""),
        QueryOutcome::NotFound(_) => (404, "ResourceNotFoundException"),
        QueryOutcome::NotActive(_) => (409, "ResourceInUseException"),
        QueryOutcome::UnmintedNextToken(_) => (400, "InvalidNextTokenException"),
        QueryOutcome::Unqueryable(_) => (400, "InvalidInputException"),
    }
}

/// One operation of this service: the resource kind its ARN member resolves, an ARN of
/// that kind, and how the control plane publishes such a resource.
struct Op {
    /// The `X-Amz-Target` operation name.
    name: &'static str,
    /// The request member the ARN arrives in.
    member: &'static str,
    /// The resource kind that member must name — the table the shared decision is
    /// computed from.
    kind: ResourceKind,
    /// The ARN resource field of a resource of that kind.
    resource: &'static str,
    /// Publish `stored` as the control plane does for this operation's resource kind.
    put: fn(&InMemoryEngine, StoredForecast),
}

fn operations() -> [Op; 2] {
    [
        Op {
            name: "QueryForecast",
            member: "ForecastArn",
            kind: FORECAST,
            resource: "forecast/electricity_demand",
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
            put: |engine, stored| {
                engine
                    .state::<ForecastQueryState>()
                    .lock()
                    .put_what_if_forecast(stored);
            },
        },
    ]
}

/// Which series the stored resource carries — one per shape of filter answer a row needs.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// A single series identified by `item_id` alone.
    OneSeries,
    /// Two series sharing an `item_id` and told apart by a `store` dimension, so a filter
    /// on `item_id` alone selects both.
    TwoSeriesSharingTheItem,
}

/// The daily timestamps a stored series carries.
const TIMESTAMPS: [&str; 3] = [
    "2015-01-01T00:00:00",
    "2015-01-02T00:00:00",
    "2015-01-03T00:00:00",
];

/// Points over [`TIMESTAMPS`] starting at `base`, rising by 1.5 a day.
fn points(base: f64) -> Vec<StoredPoint> {
    TIMESTAMPS
        .iter()
        .enumerate()
        .map(|(day, timestamp)| StoredPoint::new(*timestamp, base + 1.5 * day as f64))
        .collect()
}

/// One request, and the store it is made against.
struct Row {
    case: &'static str,
    /// Does the store hold the resource the ARN names?
    stored: bool,
    /// How far off the creation run's due instant is. Zero means already readable; a
    /// span long enough that it cannot elapse while the row runs means not readable yet.
    due_in: Duration,
    shape: Shape,
    filters: &'static [(&'static str, &'static str)],
    next_token: Option<&'static str>,
    start_date: Option<&'static str>,
    end_date: Option<&'static str>,
}

/// The rule's whole surface: each row reaches a different branch of the shared decision,
/// and the rows that carry several questions at once are what pin the ORDER the branches
/// are judged in.
fn rows() -> Vec<Row> {
    let base = Row {
        case: "",
        stored: true,
        due_in: Duration::ZERO,
        shape: Shape::OneSeries,
        filters: &[("item_id", "client_21")],
        next_token: None,
        start_date: None,
        end_date: None,
    };
    vec![
        Row {
            case: "readable, the whole date range",
            ..base
        },
        Row {
            case: "readable, a window that clips the series",
            start_date: Some("2015-01-02T00:00:00"),
            end_date: Some("2015-01-03T00:00:00"),
            ..base
        },
        Row {
            case: "readable, a window that selects no point",
            start_date: Some("2030-01-01T00:00:00"),
            end_date: Some("2030-01-06T00:00:00"),
            ..base
        },
        Row {
            case: "readable, two filter criteria naming one series",
            shape: Shape::TwoSeriesSharingTheItem,
            filters: &[("item_id", "client_21"), ("store", "store_7")],
            ..base
        },
        Row {
            case: "the store holds nothing under the ARN",
            stored: false,
            ..base
        },
        Row {
            case: "the store holds nothing under the ARN, plus an unminted token",
            stored: false,
            next_token: Some("bogus-next-token"),
            ..base
        },
        Row {
            case: "the store holds nothing under the ARN, plus filters matching nothing",
            stored: false,
            filters: &[("region", "eu_central")],
            ..base
        },
        Row {
            case: "stored, creation run not due yet",
            due_in: Duration::from_secs(60),
            ..base
        },
        Row {
            case: "stored, creation run not due yet, plus an unminted token",
            due_in: Duration::from_secs(60),
            next_token: Some("bogus-next-token"),
            ..base
        },
        Row {
            case: "stored, creation run not due yet, plus filters matching no series",
            due_in: Duration::from_secs(60),
            filters: &[("item_id", "client_99")],
            ..base
        },
        Row {
            case: "readable, plus an unminted token",
            next_token: Some("x"),
            ..base
        },
        Row {
            // A token shaped like an opaque handle rather than a word, and far longer,
            // because no token this service could have minted exists whatever its form.
            case: "readable, plus a base64-shaped unminted token",
            next_token: Some("eyJhcm4iOiJmb3JlY2FzdCIsIm9mZnNldCI6M30="),
            ..base
        },
        Row {
            case: "readable, plus an unminted token and filters matching no series",
            next_token: Some("bogus-next-token"),
            filters: &[("item_id", "client_99")],
            ..base
        },
        Row {
            case: "readable, plus an unminted token and an inverted window",
            next_token: Some("bogus-next-token"),
            start_date: Some("2015-01-03T00:00:00"),
            end_date: Some("2015-01-01T00:00:00"),
            ..base
        },
        Row {
            case: "readable, filters matching no stored series",
            filters: &[("item_id", "client_99")],
            ..base
        },
        Row {
            case: "readable, filters naming a dimension this resource does not carry",
            filters: &[("region", "eu_central")],
            ..base
        },
        Row {
            case: "readable, filters matching more than one series",
            shape: Shape::TwoSeriesSharingTheItem,
            ..base
        },
        Row {
            case: "readable, an inverted window",
            start_date: Some("2015-01-03T00:00:00"),
            end_date: Some("2015-01-01T00:00:00"),
            ..base
        },
    ]
}

impl Row {
    /// The resource this row's store holds, due to become readable at `active_at`.
    fn forecast(&self, resource_arn: &str, active_at: SystemTime) -> StoredForecast {
        let stored = StoredForecast::new(resource_arn, "electricity_demand", "D", ["0.5", "mean"]);
        let stored = match self.shape {
            Shape::OneSeries => stored.with_series(
                StoredSeries::new([("item_id", "client_21")])
                    .with_statistic("p50", points(20.0))
                    .with_statistic("mean", points(21.0)),
            ),
            Shape::TwoSeriesSharingTheItem => stored
                .with_series(
                    StoredSeries::new([("item_id", "client_21"), ("store", "store_7")])
                        .with_statistic("p50", points(20.0))
                        .with_statistic("mean", points(21.0)),
                )
                .with_series(
                    StoredSeries::new([("item_id", "client_21"), ("store", "store_8")])
                        .with_statistic("p50", points(30.0))
                        .with_statistic("mean", points(31.0)),
                ),
        };
        stored.active_from(active_at)
    }

    /// This row's `Filters`, as the shared decision reads them.
    fn filter_map(&self) -> HashMap<String, String> {
        self.filters
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    /// This row's request body for `op`.
    fn body(&self, op: &Op, queried: &str) -> Value {
        let mut body = json!({op.member: queried, "Filters": self.filter_map()});
        let object = body.as_object_mut().expect("request body is an object");
        for (member, value) in [
            ("NextToken", self.next_token),
            ("StartDate", self.start_date),
            ("EndDate", self.end_date),
        ] {
            if let Some(value) = value {
                object.insert(member.to_string(), json!(value));
            }
        }
        body
    }
}

/// Both operations answer exactly the outcome the shared decision computed — the same
/// status, the same wire code and the same message — and agree with each other on every
/// row.
#[tokio::test]
async fn both_query_operations_render_the_one_shared_post_resolution_decision() {
    for row in rows() {
        // What each operation answered, so the two can be compared against each other as
        // well as against the shared decision.
        let mut answers: Vec<(&'static str, u16, String)> = Vec::new();

        for op in operations() {
            let (engine, router) = harness();
            let queried = arn(op.resource);
            // One instant for the whole row: the resource's due instant and the one the
            // shared decision is evaluated at are the same clock the handler reads.
            let stored = row.forecast(&queried, SystemTime::now() + row.due_in);
            if row.stored {
                (op.put)(&engine, stored.clone());
            }

            let filters = row.filter_map();
            let expected = op.kind.resolve_query(
                row.stored.then_some(&stored),
                &QueryRequest {
                    arn: &queried,
                    filters: &filters,
                    start_date: row.start_date,
                    end_date: row.end_date,
                    next_token: row.next_token,
                },
                SystemTime::now(),
            );
            let (status_wanted, code_wanted) = rendering(&expected);

            let case = format!("{} / {}", op.name, row.case);
            let (status, response) = call(&router, op.name, &row.body(&op, &queried)).await;
            assert_eq!(
                status, status_wanted,
                "case {case:?}: the shared decision answered {expected:?}, so expected \
                 HTTP {status_wanted}, got {status} with {response}"
            );

            match &expected {
                QueryOutcome::Predictions(predictions) => {
                    assert_eq!(
                        response_predictions(&response),
                        outcome_predictions(predictions),
                        "case {case:?}: expected the projection the shared decision \
                         computed, got {response}"
                    );
                }
                refusal => {
                    let actual_code = error_code(&response);
                    assert_eq!(
                        actual_code, code_wanted,
                        "case {case:?}: the shared decision answered {refusal:?}, so \
                         expected {code_wanted}, got {actual_code:?} with {response}"
                    );
                    let actual = error_message(&response);
                    assert_eq!(
                        Some(actual.as_str()),
                        refusal.message(),
                        "case {case:?}: expected the message the shared decision worded, \
                         got {actual:?}"
                    );
                }
            }

            answers.push((op.name, status, error_code(&response)));
        }

        // The point of hoisting the decision: the two operations cannot drift into
        // different conditions or a different order, so every row earns one status and
        // one code whichever operation is called.
        let (first_name, first_status, first_code) = answers[0].clone();
        for (name, status, code) in &answers[1..] {
            assert_eq!(
                (*status, code.as_str()),
                (first_status, first_code.as_str()),
                "case {:?}: {name} answered HTTP {status} {code:?} where {first_name} \
                 answered HTTP {first_status} {first_code:?}; both render one shared \
                 decision",
                row.case
            );
        }
    }
}

/// The answered path, against points written out rather than computed, so a 200 row of
/// the table above cannot pass on a projection that merely agrees with itself: both
/// operations report the stored series of every statistic their resource was created
/// with, ascending, clipped to the window the request asked for.
#[tokio::test]
async fn the_answered_projection_is_the_stored_series_of_each_statistic() {
    // The stored series: `p50` from 20.0 and `mean` from 21.0, rising 1.5 a day over
    // [`TIMESTAMPS`] — spelled out here instead of derived.
    let whole_range: BTreeMap<String, Vec<(String, f64)>> = [
        (
            "p50".to_string(),
            vec![
                ("2015-01-01T00:00:00".to_string(), 20.0),
                ("2015-01-02T00:00:00".to_string(), 21.5),
                ("2015-01-03T00:00:00".to_string(), 23.0),
            ],
        ),
        (
            "mean".to_string(),
            vec![
                ("2015-01-01T00:00:00".to_string(), 21.0),
                ("2015-01-02T00:00:00".to_string(), 22.5),
                ("2015-01-03T00:00:00".to_string(), 24.0),
            ],
        ),
    ]
    .into_iter()
    .collect();
    let last_two_days: BTreeMap<String, Vec<(String, f64)>> = whole_range
        .iter()
        .map(|(statistic, points)| (statistic.clone(), points[1..].to_vec()))
        .collect();
    let nothing: BTreeMap<String, Vec<(String, f64)>> = whole_range
        .keys()
        .map(|statistic| (statistic.clone(), Vec::new()))
        .collect();

    for op in operations() {
        for (case, window, expected) in [
            ("no window: the complete date range", json!({}), &whole_range),
            (
                "a window starting on the second day",
                json!({"StartDate": "2015-01-02T00:00:00"}),
                &last_two_days,
            ),
            (
                "a window the series has no point in",
                json!({"StartDate": "2030-01-01T00:00:00", "EndDate": "2030-01-06T00:00:00"}),
                &nothing,
            ),
        ] {
            let (engine, router) = harness();
            let queried = arn(op.resource);
            let row = Row {
                case,
                stored: true,
                due_in: Duration::ZERO,
                shape: Shape::OneSeries,
                filters: &[("item_id", "client_21")],
                next_token: None,
                start_date: None,
                end_date: None,
            };
            (op.put)(&engine, row.forecast(&queried, SystemTime::now()));

            let mut body = json!({op.member: &queried, "Filters": row.filter_map()});
            let object = body.as_object_mut().expect("request body is an object");
            for (member, value) in window.as_object().expect("the window is an object") {
                object.insert(member.clone(), value.clone());
            }

            let case = format!("{} / {case}", op.name);
            let (status, response) = call(&router, op.name, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {response}"
            );
            let actual = response_predictions(&response);
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The token refusal's sentence, written out per operation rather than taken from the
/// code under test: the documented sentence of the modelled `InvalidNextTokenException`
/// first, then why this operation has no token to resume from.
#[tokio::test]
async fn the_unminted_token_refusal_states_the_documented_sentence_then_this_operations_reason() {
    let expected_per_operation = [
        "The token is not valid. Tokens expire after 24 hours. QueryForecast returns the \
         complete date range for the filtered forecast, so it returns no NextToken to \
         resume from.",
        "The token is not valid. Tokens expire after 24 hours. QueryWhatIfForecast \
         returns the complete date range for the filtered what-if forecast, so it \
         returns no NextToken to resume from.",
    ];

    for (op, expected) in operations().into_iter().zip(expected_per_operation) {
        let (engine, router) = harness();
        let queried = arn(op.resource);
        let row = Row {
            case: "a readable resource plus an unminted token",
            stored: true,
            due_in: Duration::ZERO,
            shape: Shape::OneSeries,
            filters: &[("item_id", "client_21")],
            next_token: Some("bogus-next-token"),
            start_date: None,
            end_date: None,
        };
        (op.put)(&engine, row.forecast(&queried, SystemTime::now()));

        let case = format!("{} / {}", op.name, row.case);
        let (status, response) = call(&router, op.name, &row.body(&op, &queried)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {response}"
        );
        let actual = error_message(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The ORDER is part of the decision, so it is stated once too: each branch is reached
/// only once every branch ahead of it has passed, which is what a request carrying every
/// question at once shows.
#[tokio::test]
async fn the_shared_decision_judges_each_branch_only_once_the_earlier_ones_pass() {
    let filters: HashMap<String, String> = [("region".to_string(), "eu_central".to_string())]
        .into_iter()
        .collect();

    for op in operations() {
        let queried = arn(op.resource);
        let pending = StoredForecast::new(&queried, "electricity_demand", "D", ["0.5"])
            .with_series(StoredSeries::new([("item_id", "client_21")]).with_statistic("p50", points(20.0)))
            .active_from(SystemTime::now() + Duration::from_secs(60));
        let readable = pending.clone().active_from(SystemTime::now());

        // A request that asks every question at once: an absent resource, a resource that
        // is not readable, an unminted token and filters this resource has no answer for.
        let request = QueryRequest {
            arn: &queried,
            filters: &filters,
            start_date: None,
            end_date: None,
            next_token: Some("bogus-next-token"),
        };
        let now = SystemTime::now();

        for (case, stored, expected) in [
            (
                "nothing stored under the ARN",
                None,
                QueryOutcome::NotFound(op.kind.not_found_message(&queried)),
            ),
            (
                "stored, creation run not due yet",
                Some(&pending),
                QueryOutcome::NotActive(op.kind.not_active_message(&queried)),
            ),
            (
                "stored and readable",
                Some(&readable),
                QueryOutcome::UnmintedNextToken(op.kind.unminted_next_token_message()),
            ),
        ] {
            let case = format!("{} / {case}", op.name);
            let actual = op.kind.resolve_query(stored, &request, now);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
