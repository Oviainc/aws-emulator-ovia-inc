// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `MetricReferences` `ListServiceDependencies` reports over
//! HTTP, for every kind of edge the store can be evidence of.
//!
//! The rule under test: every reported `ServiceDependency` carries the standard metrics
//! Application Signals collects for THAT call — `Latency`, `Fault` and `Error` in the
//! `ApplicationSignals` namespace — dimensioned by the calling entity's identity and
//! operation plus the called entity's identity and operation, so no reported edge carries
//! an empty list, two edges out of one service carry different dimensions, and a dimension
//! whose attribute the stored identity does not carry is left out rather than reported
//! empty. The paged view reports the same references as the single-page one, since a page
//! is a window onto the same projection.
//!
//! Every read drives the real `restJson1` surface the router production mounts, so the
//! bodies asserted here are the ones a client sees. (The harness below is this file's own,
//! as in every other suite in this crate; the expected metric catalogue comes from the
//! suite that owns that rule, so it is written out once for the crate.)

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The expected metric catalogue, and the rendering of one answer's references and
// dimensions, come from the suite that owns that rule, so this crate's tests state them
// ONCE.
use super::discovered_metric_references::{dimensions_of, metric_set, standard_metrics};

/// The SigV4 credential scope routes the request to this service. Signature verification
/// is off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

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

/// The window every read here covers: the stored SLOs are created inside it.
fn covering_window() -> String {
    let now = now_seconds();
    format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600)
}

/// Create one SLO and assert it was stored.
async fn create_slo(router: &MultiServiceRouter, case: &str, body: Value) {
    let (status, answer) = call(router, "POST", "/slo", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO create to answer 200, got {status} with {answer:?}"
    );
}

/// The identity of one Application Signals entity, by `Type`.
fn service_entity(name: &str, environment: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": environment })
}

/// The far end of an edge into a remote service.
fn remote_service(name: &str, environment: &str) -> Value {
    json!({ "Type": "RemoteService", "Name": name, "Environment": environment })
}

/// The far end of an edge into a remote resource, which `Type` says is identified by its
/// `ResourceType` and `Identifier` rather than by a `Name`.
fn remote_resource(resource_type: &str, identifier: &str, environment: &str) -> Value {
    json!({
        "Type": "Resource",
        "ResourceType": resource_type,
        "Identifier": identifier,
        "Environment": environment
    })
}

/// A seeded SLO: the entity members under the SLI form `request_based` selects.
fn slo_body(name: &str, request_based: bool, metric_config: Value) -> Value {
    if request_based {
        json!({
            "Name": name,
            "RequestBasedSliConfig": { "RequestBasedSliMetricConfig": metric_config }
        })
    } else {
        json!({ "Name": name, "SliConfig": { "SliMetricConfig": metric_config } })
    }
}

/// One row of the table every test here reads back: an edge the store is evidence of, and
/// the dimensions every metric reference of that edge must carry.
///
/// The rows span what the rule generalises over: two edges out of ONE operation of one
/// service, a second operation of that same service, the `SliConfig` and the
/// `RequestBasedSliConfig` form of the same entity members, a far end identified as a remote
/// SERVICE (by `Name`) and one identified as a remote RESOURCE (by `ResourceType` and
/// `Identifier`), a far end whose identity names no `Environment`, a caller in a second
/// environment, and a caller that is itself a resource, so its identity names no `Name`.
///
/// The dimension names are the ones `src/state.rs` maps the `Attributes` entries onto
/// (`NEAR_END_DIMENSIONS`, `FAR_END_DIMENSIONS`, `OPERATION_DIMENSION`,
/// `REMOTE_OPERATION_DIMENSION`).
struct Row {
    case: &'static str,
    /// The `KeyAttributes` the read names the calling entity with.
    caller: Value,
    /// `OperationName` of the reported edge.
    operation: &'static str,
    /// `DependencyKeyAttributes` of the reported edge.
    callee: Value,
    /// `DependencyOperationName` of the reported edge.
    callee_operation: &'static str,
    /// The dimension set every reference of the edge carries.
    dimensions: Vec<String>,
}

/// Seed the store with every edge the rows below read back, plus one SLO that names a
/// dependency but no operation of its own — which records no edge, since the modelled
/// `ServiceDependency.OperationName` is `@required`.
async fn seed(router: &MultiServiceRouter) {
    create_slo(
        router,
        "checkout -> payments",
        slo_body(
            "checkout-payments",
            false,
            json!({
                "KeyAttributes": service_entity("checkout", "prod"),
                "OperationName": "POST /checkout",
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": remote_service("payments", "prod"),
                    "DependencyOperationName": "Charge"
                }
            }),
        ),
    )
    .await;
    create_slo(
        router,
        "checkout -> fraud, a second edge out of the SAME operation",
        slo_body(
            "checkout-fraud",
            false,
            json!({
                "KeyAttributes": service_entity("checkout", "prod"),
                "OperationName": "POST /checkout",
                "MetricType": "AVAILABILITY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": remote_service("fraud", "prod"),
                    "DependencyOperationName": "Score"
                }
            }),
        ),
    )
    .await;
    create_slo(
        router,
        "checkout -> carts table, from a second operation, request-based",
        slo_body(
            "checkout-carts",
            true,
            json!({
                "KeyAttributes": service_entity("checkout", "prod"),
                "OperationName": "GET /cart",
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": remote_resource(
                        "AWS::DynamoDB::Table", "carts", "prod"
                    ),
                    "DependencyOperationName": "GetItem"
                }
            }),
        ),
    )
    .await;
    create_slo(
        router,
        "billing -> tax, a caller in a second environment and a far end that names none",
        slo_body(
            "billing-tax",
            true,
            json!({
                "KeyAttributes": service_entity("billing", "staging"),
                "OperationName": "POST /invoice",
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "tax" },
                    "DependencyOperationName": "Quote"
                }
            }),
        ),
    )
    .await;
    create_slo(
        router,
        "a resource caller, whose identity names no Name",
        slo_body(
            "ship-fn-rates",
            false,
            json!({
                "KeyAttributes": {
                    "Type": "Resource",
                    "ResourceType": "AWS::Lambda::Function",
                    "Identifier": "ship-fn",
                    "Environment": "prod"
                },
                "OperationName": "Invoke",
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": remote_service("rates", "prod"),
                    "DependencyOperationName": "Quote"
                }
            }),
        ),
    )
    .await;
    create_slo(
        router,
        "a dependency named with no operation of the caller's own",
        slo_body(
            "support-crm",
            false,
            json!({
                "KeyAttributes": service_entity("support", "prod"),
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": remote_service("crm", "prod"),
                    "DependencyOperationName": "Lookup"
                }
            }),
        ),
    )
    .await;
}

/// The expected edges, one row per edge the seed records.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "checkout -> payments, an edge into a remote service",
            caller: service_entity("checkout", "prod"),
            operation: "POST /checkout",
            callee: remote_service("payments", "prod"),
            callee_operation: "Charge",
            dimensions: dimensions(&[
                "Environment=prod",
                "Service=checkout",
                "Operation=POST /checkout",
                "RemoteService=payments",
                "RemoteOperation=Charge",
            ]),
        },
        Row {
            case: "checkout -> fraud, a second edge out of one operation",
            caller: service_entity("checkout", "prod"),
            operation: "POST /checkout",
            callee: remote_service("fraud", "prod"),
            callee_operation: "Score",
            dimensions: dimensions(&[
                "Environment=prod",
                "Service=checkout",
                "Operation=POST /checkout",
                "RemoteService=fraud",
                "RemoteOperation=Score",
            ]),
        },
        Row {
            case: "checkout -> carts, an edge into a remote resource, request-based",
            caller: service_entity("checkout", "prod"),
            operation: "GET /cart",
            callee: remote_resource("AWS::DynamoDB::Table", "carts", "prod"),
            callee_operation: "GetItem",
            // The far end names no `Name`, so it contributes the resource dimensions its
            // `Type` does define instead of `RemoteService`.
            dimensions: dimensions(&[
                "Environment=prod",
                "Service=checkout",
                "Operation=GET /cart",
                "RemoteResourceType=AWS::DynamoDB::Table",
                "RemoteResourceIdentifier=carts",
                "RemoteOperation=GetItem",
            ]),
        },
        Row {
            case: "billing -> tax, a far end that names no Environment",
            caller: service_entity("billing", "staging"),
            operation: "POST /invoice",
            callee: json!({ "Type": "RemoteService", "Name": "tax" }),
            callee_operation: "Quote",
            dimensions: dimensions(&[
                "Environment=staging",
                "Service=billing",
                "Operation=POST /invoice",
                "RemoteService=tax",
                "RemoteOperation=Quote",
            ]),
        },
        Row {
            case: "a resource caller, whose identity names no Name",
            caller: json!({
                "Type": "Resource",
                "ResourceType": "AWS::Lambda::Function",
                "Identifier": "ship-fn",
                "Environment": "prod"
            }),
            operation: "Invoke",
            callee: remote_service("rates", "prod"),
            callee_operation: "Quote",
            dimensions: dimensions(&[
                "Environment=prod",
                "Operation=Invoke",
                "RemoteService=rates",
                "RemoteOperation=Quote",
            ]),
        },
    ]
}

/// The expected dimensions of one row, written the way the row states them.
fn dimensions(expected: &[&str]) -> Vec<String> {
    expected.iter().map(ToString::to_string).collect()
}

/// Read every dependency of `caller` over the covering window, following `NextToken` with
/// the page size `max_results` when one is given.
async fn dependencies_of(
    router: &MultiServiceRouter,
    caller: &Value,
    max_results: Option<u32>,
) -> Vec<Value> {
    let window = covering_window();
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut uri = format!("/service-dependencies?{window}");
        if let Some(max_results) = max_results {
            uri.push_str(&format!("&MaxResults={max_results}"));
        }
        if let Some(token) = token.as_deref() {
            uri.push_str(&format!("&NextToken={token}"));
        }
        let (status, answer) = call(
            router,
            "POST",
            &uri,
            Some(json!({ "KeyAttributes": caller.clone() })),
        )
        .await;
        assert_eq!(
            status, 200,
            "caller {caller:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        let page = answer["ServiceDependencies"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("caller {caller:?}: ServiceDependencies is @required, got {answer:?}")
            });
        if let Some(max_results) = max_results {
            assert!(
                page.len() <= max_results as usize,
                "caller {caller:?}: expected at most MaxResults={max_results} entries, got {}",
                page.len()
            );
        }
        walked.extend(page);
        token = answer["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() <= EXPECTED_EDGE_CEILING,
            "caller {caller:?}: the cursor did not terminate, walked {} entries",
            walked.len()
        );
    }
    walked
}

/// A walk over a seeded caller's edges cannot exceed this; a cursor that does is looping.
const EXPECTED_EDGE_CEILING: usize = 32;

/// The `OperationName|DependencyKeyAttributes|DependencyOperationName` identity of one
/// reported edge, used to match a reported entry to the row it is about.
fn edge_identity(operation: &str, callee: &Value, callee_operation: &str) -> String {
    let mut attributes: Vec<String> = callee
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    attributes.sort();
    format!("{operation}|{}|{callee_operation}", attributes.join(","))
}

/// The identity of a reported entry, as [`edge_identity`] renders a row's.
fn reported_identity(entry: &Value) -> String {
    edge_identity(
        entry["OperationName"].as_str().unwrap_or_default(),
        &entry["DependencyKeyAttributes"],
        entry["DependencyOperationName"].as_str().unwrap_or_default(),
    )
}

/// Every reported edge carries the standard metrics, dimensioned by that edge: the calling
/// entity's identity and operation plus the called entity's identity and operation.
#[tokio::test]
async fn every_reported_edge_carries_the_metrics_of_that_edge() {
    let router = router();
    seed(&router).await;

    for row in rows() {
        let reported = dependencies_of(&router, &row.caller, None).await;
        let wanted = edge_identity(row.operation, &row.callee, row.callee_operation);
        let entry = reported
            .iter()
            .find(|entry| reported_identity(entry) == wanted)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the stored edge {wanted:?} among {reported:?}",
                    row.case
                )
            });

        assert_eq!(
            metric_set(&entry["MetricReferences"]),
            standard_metrics(),
            "case {:?}: expected the standard metrics, got {:?}",
            row.case,
            entry["MetricReferences"]
        );
        let expected: BTreeSet<String> = row.dimensions.iter().cloned().collect();
        for reference in entry["MetricReferences"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            assert_eq!(
                dimensions_of(&reference),
                expected,
                "case {:?}: expected the edge's own dimensions on {:?}, got {:?}",
                row.case,
                reference["MetricName"],
                reference["Dimensions"]
            );
            assert!(
                reference.get("AccountId").is_none(),
                "case {:?}: a same-account metric names no source account, got {reference:?}",
                row.case
            );
        }
    }
}

/// The reported edges of one caller are exactly the edges the store records for it: an SLO
/// that names a dependency but no operation of the caller's own records none, and each
/// reported edge carries a dimension set of its own, so the metrics of two edges out of one
/// service are never the same metrics.
#[tokio::test]
async fn one_caller_reports_one_entry_per_recorded_edge() {
    let router = router();
    seed(&router).await;

    // Per caller: the identities of the edges the seed records for it. `support` names a
    // dependency with no operation of its own, so it records no edge at all.
    let callers: Vec<(&str, Value, usize)> = vec![
        ("checkout, three recorded edges", service_entity("checkout", "prod"), 3),
        (
            "billing, one recorded edge",
            service_entity("billing", "staging"),
            1,
        ),
        (
            "support, a dependency with no operation of its own",
            service_entity("support", "prod"),
            0,
        ),
        ("an identity no SLO monitors", service_entity("ghost", "prod"), 0),
    ];

    for (case, caller, expected) in callers {
        let reported = dependencies_of(&router, &caller, None).await;
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} edges, got {reported:?}"
        );

        let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
        for entry in &reported {
            let references = entry["MetricReferences"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !references.is_empty(),
                "case {case:?}: expected a reported edge to carry metrics, got {entry:?}"
            );
            let dimensions = dimensions_of(&references[0]);
            assert!(
                seen.insert(dimensions.clone()),
                "case {case:?}: expected each edge's metrics to be dimensioned differently, \
                 got {dimensions:?} twice"
            );
        }
    }
}

/// A page is a window onto the same projection, so walking a caller's edges one page size at
/// a time reports the same entries, references and all, in the same order as a single page.
///
/// The page sizes span the modelled `range 1..100` of `ListServiceDependenciesMaxResults`:
/// both bounds and a size inside it.
#[tokio::test]
async fn a_paged_walk_reports_the_same_references() {
    let router = router();
    seed(&router).await;
    let caller = service_entity("checkout", "prod");
    let single_page = dependencies_of(&router, &caller, Some(100)).await;
    assert_eq!(
        single_page.len(),
        3,
        "expected checkout's three recorded edges on one page, got {single_page:?}"
    );

    for page_size in [1_u32, 2, 100] {
        let walked = dependencies_of(&router, &caller, Some(page_size)).await;
        assert_eq!(
            walked, single_page,
            "case \"MaxResults={page_size}\": expected the walk to report every edge exactly \
             once with its own references, got {walked:?}"
        );
    }

    // A size outside the modelled range is refused rather than coerced, so a caller cannot
    // page with a bound the model does not admit.
    for page_size in [0_u32, 101] {
        let window = covering_window();
        let (status, answer) = call(
            &router,
            "POST",
            &format!("/service-dependencies?{window}&MaxResults={page_size}"),
            Some(json!({ "KeyAttributes": caller.clone() })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"MaxResults={page_size}\": expected HTTP 400 for a value outside range \
             1..100, got {status} with {answer:?}"
        );
    }
}
