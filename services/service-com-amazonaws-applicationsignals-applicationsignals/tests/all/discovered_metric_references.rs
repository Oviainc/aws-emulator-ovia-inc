// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `MetricReferences` every topology read reports over HTTP:
//! `GetService`, `ListServices`, `ListServiceOperations`, `ListServiceDependencies` and
//! `ListServiceDependents`.
//!
//! The rule under test: a `MetricReferences` member reports the standard metrics
//! Application Signals collects for the entity or edge the entry describes — `Latency`,
//! `Fault` and `Error` in the `ApplicationSignals` namespace — dimensioned by THAT entry's
//! stored identity, so no reported entry carries an empty list and two entries carry
//! different dimensions exactly as their stored identities differ. An identity the store is
//! no evidence of over the window is not discovered, so it carries no reference at all.
//!
//! Every read drives the real `restJson1` surface the router production mounts, so the
//! bodies asserted here are the ones a client sees. (The harness below is this file's own,
//! as in every other suite in this crate: the suites are separate modules of one test
//! target and none of them exports its helpers.)

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification
/// is off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The namespace the standard metrics live in (`src/state.rs`,
/// `APPLICATION_SIGNALS_NAMESPACE`).
const NAMESPACE: &str = "ApplicationSignals";

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Seconds since the epoch, for a window around the instant the stored SLOs are created at.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// Create one SLO and assert it was stored.
async fn create_slo(router: &MultiServiceRouter, case: &str, body: Value) {
    let (status, answer) = call(router, "POST", "/slo", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO create to answer 200, got {status} with {answer:?}"
    );
}

/// `Namespace|MetricName|MetricType` per reference, as a set. Shared with the suites that
/// assert the same catalogue on other reads (`time_ranged_reads`, `list_services`), so the
/// expected metrics are written out ONCE for this crate's tests.
pub fn metric_set(references: &Value) -> BTreeSet<String> {
    references
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|reference| {
                    format!(
                        "{}|{}|{}",
                        reference["Namespace"].as_str().unwrap_or_default(),
                        reference["MetricName"].as_str().unwrap_or_default(),
                        reference["MetricType"].as_str().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The standard metrics the catalogue names, as [`metric_set`] renders them: the three
/// metrics Application Signals collects for every entity it discovers (`src/state.rs`,
/// `STANDARD_METRICS`).
pub fn standard_metrics() -> BTreeSet<String> {
    ["Latency|LATENCY", "Fault|FAULT", "Error|ERROR"]
        .into_iter()
        .map(|metric| format!("{NAMESPACE}|{metric}"))
        .collect()
}

/// The `Name=Value` dimensions of ONE reference, as a set. Shared with the suite that
/// asserts the same dimension rule on `ListServiceDependencies`
/// (`service_dependency_metrics`), so one answer's references are rendered in ONE place.
pub fn dimensions_of(reference: &Value) -> BTreeSet<String> {
    reference["Dimensions"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|dimension| {
                    format!(
                        "{}={}",
                        dimension["Name"].as_str().unwrap_or_default(),
                        dimension["Value"].as_str().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The expected dimension set, written the way a row states it.
fn dimension_set(dimensions: &[&str]) -> BTreeSet<String> {
    dimensions.iter().map(ToString::to_string).collect()
}

/// Assert that `entry`'s `MetricReferences` is the standard catalogue, every reference of it
/// dimensioned by `expected`.
fn assert_references(case: &str, entry: &Value, expected: &BTreeSet<String>) {
    assert_eq!(
        metric_set(&entry["MetricReferences"]),
        standard_metrics(),
        "case {case:?}: expected the standard metrics, got {:?}",
        entry["MetricReferences"]
    );
    let references = entry["MetricReferences"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for reference in &references {
        assert_eq!(
            &dimensions_of(reference),
            expected,
            "case {case:?}: expected the entry's own dimensions on {:?}, got {:?}",
            reference["MetricName"],
            reference["Dimensions"]
        );
        assert!(
            reference.get("AccountId").is_none(),
            "case {case:?}: a same-account metric names no source account, got {reference:?}"
        );
    }
}

/// The identity of one Application Signals entity, by `Type`.
fn service_entity(name: &str, environment: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": environment })
}

/// Store the SLOs every test here reads back.
///
/// They span what the rule generalises over: a service with TWO operations, an edge into a
/// remote SERVICE, an edge into a remote RESOURCE (whose far end is identified by
/// `ResourceType`/`Identifier` rather than by `Name`), an entity whose identity names no
/// `Environment`, and an environment that differs between two services.
async fn seed(router: &MultiServiceRouter) {
    create_slo(
        router,
        "checkout calls payments",
        json!({
            "Name": "checkout-payments",
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": service_entity("checkout", "prod"),
                "OperationName": "POST /checkout",
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": {
                        "Type": "RemoteService", "Name": "payments", "Environment": "prod"
                    },
                    "DependencyOperationName": "Charge"
                }
            }}
        }),
    )
    .await;
    create_slo(
        router,
        "a second operation of checkout",
        json!({
            "Name": "checkout-cart",
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": service_entity("checkout", "prod"),
                "OperationName": "GET /cart",
                "MetricType": "AVAILABILITY"
            }}
        }),
    )
    .await;
    create_slo(
        router,
        "orders calls a resource",
        json!({
            "Name": "orders-table",
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": service_entity("orders", "staging"),
                "OperationName": "POST /orders",
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": {
                        "Type": "Resource",
                        "ResourceType": "AWS::DynamoDB::Table",
                        "Identifier": "orders-table",
                        "Environment": "prod"
                    },
                    "DependencyOperationName": "PutItem"
                }
            }}
        }),
    )
    .await;
    create_slo(
        router,
        "an identity that names no Environment",
        json!({
            "Name": "billing-latency",
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": { "Type": "Service", "Name": "billing" },
                "MetricType": "LATENCY"
            }}
        }),
    )
    .await;
}

/// Every reported entry carries the standard metrics, dimensioned by the identity of the
/// entity or edge the entry is about.
///
/// The rows differ along the READ (all five), the kind of entry (an entity, an operation of
/// one, an edge out of one, an edge into one), the SIZE of the identity (one attribute to
/// four), the `Type` of the far end (a remote service, a remote resource) and the
/// environment the entity runs in.
#[tokio::test]
async fn every_reported_entry_carries_the_metrics_of_its_own_identity() {
    /// One read, and the dimension set every entry it reports must carry.
    struct Case {
        case: &'static str,
        method: &'static str,
        path: &'static str,
        body: Option<Value>,
        /// The output member holding the entries, or `None` for `GetService`'s one entity.
        member: Option<&'static str>,
        /// One expected dimension set per reported entry, in the order the read reports
        /// them.
        expected: Vec<BTreeSet<String>>,
    }

    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    let cases = vec![
        Case {
            case: "GetService checkout",
            method: "POST",
            path: "/service",
            body: Some(json!({ "KeyAttributes": service_entity("checkout", "prod") })),
            member: None,
            expected: vec![dimension_set(&["Environment=prod", "Service=checkout"])],
        },
        Case {
            case: "GetService billing, an identity that names no Environment",
            method: "POST",
            path: "/service",
            body: Some(json!({ "KeyAttributes": { "Type": "Service", "Name": "billing" } })),
            member: None,
            expected: vec![dimension_set(&["Service=billing"])],
        },
        Case {
            case: "ListServices",
            method: "GET",
            path: "/services",
            body: None,
            member: Some("ServiceSummaries"),
            // In the order the read reports them, which is the identity order the store
            // holds (its attributes in key order): the two `Environment`-first identities,
            // then the `Name`-first one.
            expected: vec![
                dimension_set(&["Environment=prod", "Service=checkout"]),
                dimension_set(&["Environment=staging", "Service=orders"]),
                dimension_set(&["Service=billing"]),
            ],
        },
        Case {
            case: "ListServiceOperations checkout",
            method: "POST",
            path: "/service-operations",
            body: Some(json!({ "KeyAttributes": service_entity("checkout", "prod") })),
            member: Some("ServiceOperations"),
            expected: vec![
                dimension_set(&[
                    "Environment=prod",
                    "Service=checkout",
                    "Operation=GET /cart",
                ]),
                dimension_set(&[
                    "Environment=prod",
                    "Service=checkout",
                    "Operation=POST /checkout",
                ]),
            ],
        },
        Case {
            case: "ListServiceDependencies checkout, an edge into a remote service",
            method: "POST",
            path: "/service-dependencies",
            body: Some(json!({ "KeyAttributes": service_entity("checkout", "prod") })),
            member: Some("ServiceDependencies"),
            expected: vec![dimension_set(&[
                "Environment=prod",
                "Service=checkout",
                "Operation=POST /checkout",
                "RemoteService=payments",
                "RemoteOperation=Charge",
            ])],
        },
        Case {
            case: "ListServiceDependencies orders, an edge into a remote resource",
            method: "POST",
            path: "/service-dependencies",
            body: Some(json!({ "KeyAttributes": service_entity("orders", "staging") })),
            member: Some("ServiceDependencies"),
            // The far end names no `Name`, so it contributes the resource dimensions its
            // `Type` does define instead of `RemoteService`.
            expected: vec![dimension_set(&[
                "Environment=staging",
                "Service=orders",
                "Operation=POST /orders",
                "RemoteResourceType=AWS::DynamoDB::Table",
                "RemoteResourceIdentifier=orders-table",
                "RemoteOperation=PutItem",
            ])],
        },
        Case {
            case: "ListServiceDependents payments, the invoking entity's edge",
            method: "POST",
            path: "/service-dependents",
            body: Some(json!({ "KeyAttributes": {
                "Type": "RemoteService", "Name": "payments", "Environment": "prod"
            }})),
            member: Some("ServiceDependents"),
            expected: vec![dimension_set(&[
                "Environment=prod",
                "Service=checkout",
                "Operation=POST /checkout",
                "RemoteService=payments",
                "RemoteOperation=Charge",
            ])],
        },
        Case {
            case: "ListServiceDependents on the resource end of an edge",
            method: "POST",
            path: "/service-dependents",
            body: Some(json!({ "KeyAttributes": {
                "Type": "Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "prod"
            }})),
            member: Some("ServiceDependents"),
            expected: vec![dimension_set(&[
                "Environment=staging",
                "Service=orders",
                "Operation=POST /orders",
                "RemoteResourceType=AWS::DynamoDB::Table",
                "RemoteResourceIdentifier=orders-table",
                "RemoteOperation=PutItem",
            ])],
        },
    ];

    for case in &cases {
        let uri = format!("{}?{window}", case.path);
        let (status, answer) = call(&router, case.method, &uri, case.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer:?}",
            case.case
        );
        match case.member {
            None => {
                assert_eq!(
                    case.expected.len(),
                    1,
                    "case {:?}: GetService reports exactly one entity",
                    case.case
                );
                assert_references(case.case, &answer["Service"], &case.expected[0]);
            }
            Some(member) => {
                let entries = answer[member].as_array().cloned().unwrap_or_default();
                assert_eq!(
                    entries.len(),
                    case.expected.len(),
                    "case {:?}: expected {} entries, got {:?}",
                    case.case,
                    case.expected.len(),
                    answer[member]
                );
                for (entry, expected) in entries.iter().zip(&case.expected) {
                    assert_references(case.case, entry, expected);
                }
            }
        }
    }
}

/// One edge carries ONE reference set, whichever end reports it: the metrics of a call are
/// the invoking entity's, so the dependency view and the dependent view of the same edge
/// agree reference for reference.
///
/// The rows differ along the far end's kind: a remote service and a remote resource.
#[tokio::test]
async fn both_ends_of_an_edge_report_the_same_references() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    let edges = [
        (
            "checkout -> payments",
            service_entity("checkout", "prod"),
            json!({ "Type": "RemoteService", "Name": "payments", "Environment": "prod" }),
        ),
        (
            "orders -> orders-table",
            service_entity("orders", "staging"),
            json!({
                "Type": "Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "prod"
            }),
        ),
    ];

    for (case, caller, callee) in edges {
        let (_, dependencies) = call(
            &router,
            "POST",
            &format!("/service-dependencies?{window}"),
            Some(json!({ "KeyAttributes": caller })),
        )
        .await;
        let (_, dependents) = call(
            &router,
            "POST",
            &format!("/service-dependents?{window}"),
            Some(json!({ "KeyAttributes": callee })),
        )
        .await;
        assert_eq!(
            dependencies["ServiceDependencies"][0]["MetricReferences"],
            dependents["ServiceDependents"][0]["MetricReferences"],
            "case {case:?}: expected both ends to report one reference set, got {:?} and {:?}",
            dependencies["ServiceDependencies"][0]["MetricReferences"],
            dependents["ServiceDependents"][0]["MetricReferences"]
        );
        assert_eq!(
            metric_set(&dependents["ServiceDependents"][0]["MetricReferences"]),
            standard_metrics(),
            "case {case:?}: expected the standard metrics on the dependent, got {:?}",
            dependents["ServiceDependents"][0]["MetricReferences"]
        );
    }
}

/// An identity the store is no evidence of over the window was not DISCOVERED, so
/// `GetService` reports it with a present, EMPTY `MetricReferences` — and the lists report
/// no entry at all, so there is nothing to carry references.
///
/// The rows differ along why there is no evidence: an identity no SLO names, a window that
/// ends before the SLOs existed, and a window that has not begun.
#[tokio::test]
async fn an_entity_with_no_evidence_carries_no_reference() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    let cases = [
        (
            "an identity no SLO monitors",
            service_entity("ghost", "prod"),
            format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600),
        ),
        (
            "a window that ends before the SLOs existed",
            service_entity("checkout", "prod"),
            format!("StartTime={}&EndTime={}", now - 7_200, now - 3_600),
        ),
        (
            "a window that has not begun",
            service_entity("checkout", "prod"),
            format!("StartTime={}&EndTime={}", now + 3_600, now + 7_200),
        ),
    ];

    for (case, entity, window) in cases {
        let (status, answer) = call(
            &router,
            "POST",
            &format!("/service?{window}"),
            Some(json!({ "KeyAttributes": entity })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["Service"]["MetricReferences"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected a present, empty MetricReferences, got {:?}",
            answer["Service"]["MetricReferences"]
        );

        for (member, path) in [
            ("ServiceOperations", "/service-operations"),
            ("ServiceDependencies", "/service-dependencies"),
            ("ServiceDependents", "/service-dependents"),
        ] {
            let (status, answer) = call(
                &router,
                "POST",
                &format!("{path}?{window}"),
                Some(json!({ "KeyAttributes": service_entity("ghost", "prod") })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} on {member}: expected HTTP 200, got {status} with {answer:?}"
            );
            assert_eq!(
                answer[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?} on {member}: expected no entry, got {:?}",
                answer[member]
            );
        }
    }
}
