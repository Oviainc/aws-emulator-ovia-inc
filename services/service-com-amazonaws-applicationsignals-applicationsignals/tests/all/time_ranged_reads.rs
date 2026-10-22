// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of this service's time-ranged reads over HTTP: `ListAuditFindings`,
//! `GetService`, `ListServiceOperations`, `ListServiceDependencies` and
//! `ListServiceDependents`.
//!
//! All five bind their `StartTime`/`EndTime` to the QUERY STRING (`@httpQuery`,
//! `@required`), so every test here drives the real `restJson1` surface through the router
//! production mounts: the query values are the bytes a client puts on the wire, in each of
//! the forms the members document, and the status codes and JSON bodies are the ones a
//! client sees.

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

/// Seconds since the epoch, for a window around the instant the stored SLOs are created
/// at.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": "prod" })
}

/// One `MetricDataQuery` selecting `metric_name` in `namespace`.
fn metric_query(id: &str, namespace: &str, metric_name: &str) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": {
                "Namespace": namespace,
                "MetricName": metric_name,
                "Dimensions": [{ "Name": "Service", "Value": "checkout" }]
            },
            "Period": 60,
            "Stat": "p99"
        },
        "ReturnData": true
    })
}

/// One time-ranged read: the route it answers on, the body it takes beside the query-bound
/// time range, and the output member carrying its list (`None` for `GetService`, which
/// reports one entity).
struct Read {
    operation: &'static str,
    path: &'static str,
    body: Value,
    list_member: Option<&'static str>,
}

/// Every time-ranged read, pointed at `entity`. The bodies carry only what the operation
/// requires beside the time range.
fn reads(entity: &Value) -> Vec<Read> {
    vec![
        Read {
            operation: "ListAuditFindings",
            path: "/auditFindings",
            body: json!({
                "AuditTargets": [{ "Type": "service", "Data": { "Service": {
                    "Type": "Service", "Name": "checkout", "Environment": "prod"
                }}}]
            }),
            list_member: Some("AuditFindings"),
        },
        Read {
            operation: "GetService",
            path: "/service",
            body: json!({ "KeyAttributes": entity }),
            list_member: None,
        },
        Read {
            operation: "ListServiceOperations",
            path: "/service-operations",
            body: json!({ "KeyAttributes": entity }),
            list_member: Some("ServiceOperations"),
        },
        Read {
            operation: "ListServiceDependencies",
            path: "/service-dependencies",
            body: json!({ "KeyAttributes": entity }),
            list_member: Some("ServiceDependencies"),
        },
        Read {
            operation: "ListServiceDependents",
            path: "/service-dependents",
            body: json!({ "KeyAttributes": entity }),
            list_member: Some("ServiceDependents"),
        },
    ]
}

/// Create one SLO and assert it was stored.
async fn create_slo(router: &MultiServiceRouter, case: &str, body: Value) {
    let (status, answer) = call(router, "POST", "/slo", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO create to answer 200, got {status} with {answer:?}"
    );
}

/// The `Namespace|MetricName|MetricType` of every metric reference in `references`, as a
/// set, so an assertion names the metrics rather than their order.
fn metric_set(references: &Value) -> BTreeSet<String> {
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

/// A query-bound timestamp binds in EITHER documented form — the epoch-seconds number the
/// member documents for the raw HTTP API, and the RFC-3339 `date-time` the protocol
/// defaults to — and the read answers 200 reporting the window it was asked for.
///
/// The rows differ along what the rule generalises over: the operation (so every one of
/// the five routes is exercised), the FORM of the value (integer epoch seconds, fractional
/// epoch seconds, `date-time` with its colons percent-encoded, `date-time` with raw
/// colons) and therefore the instant's own precision.
#[tokio::test]
async fn query_timestamps_bind_in_every_documented_form() {
    /// One wire form of the same window, with the instants it denotes.
    struct Form {
        case: &'static str,
        start: &'static str,
        end: &'static str,
        start_seconds: f64,
        end_seconds: f64,
    }

    // `1788660000` / `1788674401` are the instants the recorded client sent as epoch
    // seconds (issue evidence: `$.StartTime (1788660000.0)`, `$.EndTime (1788674401.0)`);
    // the `date-time` rows are the SAME instants in the protocol's default format.
    let forms = [
        Form {
            case: "epoch seconds",
            start: "1788660000",
            end: "1788674401",
            start_seconds: 1_788_660_000.0,
            end_seconds: 1_788_674_401.0,
        },
        Form {
            case: "epoch seconds with a fraction",
            start: "1788660000.5",
            end: "1788674401.25",
            start_seconds: 1_788_660_000.5,
            end_seconds: 1_788_674_401.25,
        },
        Form {
            case: "date-time, colons percent-encoded",
            start: "2026-09-06T02%3A00%3A00Z",
            end: "2026-09-06T06%3A00%3A01Z",
            start_seconds: 1_788_660_000.0,
            end_seconds: 1_788_674_401.0,
        },
        Form {
            case: "date-time, raw colons",
            start: "2026-09-06T02:00:00Z",
            end: "2026-09-06T06:00:01Z",
            start_seconds: 1_788_660_000.0,
            end_seconds: 1_788_674_401.0,
        },
    ];

    let router = router();
    let entity = service_entity("checkout");
    for form in &forms {
        for read in reads(&entity) {
            let case = format!("{} as {}", read.operation, form.case);
            let uri = format!(
                "{}?StartTime={}&EndTime={}",
                read.path, form.start, form.end
            );
            let (status, answer) = call(&router, "POST", &uri, Some(read.body.clone())).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
            );
            assert_eq!(
                answer["StartTime"].as_f64(),
                Some(form.start_seconds),
                "case {case:?}: expected the requested StartTime back, got {:?}",
                answer["StartTime"]
            );
            assert_eq!(
                answer["EndTime"].as_f64(),
                Some(form.end_seconds),
                "case {case:?}: expected the requested EndTime back, got {:?}",
                answer["EndTime"]
            );
            match read.list_member {
                // Nothing is stored, so every list is reported empty — present and empty,
                // not absent: each is a `@required` output member.
                Some(member) => assert_eq!(
                    answer[member].as_array().map(Vec::len),
                    Some(0),
                    "case {case:?}: expected an empty {member}, got {:?}",
                    answer[member]
                ),
                // `GetService` reports the entity the request identified, with no metric
                // reference for an entity nothing monitors.
                None => {
                    assert_eq!(
                        answer["Service"]["KeyAttributes"], entity,
                        "case {case:?}: expected the requested KeyAttributes back, got {:?}",
                        answer["Service"]["KeyAttributes"]
                    );
                    assert_eq!(
                        answer["Service"]["MetricReferences"].as_array().map(Vec::len),
                        Some(0),
                        "case {case:?}: expected no metric reference, got {:?}",
                        answer["Service"]["MetricReferences"]
                    );
                }
            }
        }
    }
}

/// A query value in a form NEITHER documented form covers is still refused: the service
/// reads the epoch-seconds number and the RFC-3339 `date-time`, and nothing else becomes a
/// third accepted encoding.
///
/// The rows differ along the kind of value: scientific notation, an explicit `+` sign, a
/// bare fraction, a non-numeric word and an empty value.
#[tokio::test]
async fn a_query_value_in_no_documented_form_is_refused() {
    let cases = [
        ("scientific notation", "1.7886e9"),
        ("an explicit plus sign", "+1788660000"),
        ("a bare fractional part", ".5"),
        ("a word", "yesterday"),
        ("nothing at all", ""),
    ];

    let router = router();
    let entity = service_entity("checkout");
    for (case, value) in cases {
        for read in reads(&entity) {
            let case = format!("{} with {case}", read.operation);
            let uri = format!("{}?StartTime={value}&EndTime=1788674401", read.path);
            let (status, answer) = call(&router, "POST", &uri, Some(read.body.clone())).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
            );
        }
    }
}

/// A time range that does not run forwards is refused with `ValidationException` (HTTP
/// 400) naming BOTH instants, on every operation that takes one — whichever form the
/// instants were sent in.
///
/// The rows differ along the relation between the bounds (inverted, equal), the instants
/// themselves (so the stated rendering is pinned on four different calendar dates) and the
/// wire form they were sent in.
#[tokio::test]
async fn a_range_that_does_not_run_forwards_is_refused() {
    /// One refused range and the message it earns.
    struct Case {
        case: &'static str,
        start: &'static str,
        end: &'static str,
        message: &'static str,
    }

    // The instants are stated the way the real service states them — the `java.util.Date`
    // rendering, in UTC. The first row is the recorded refusal quoted in the issue
    // (`start 1788660000`, `end 1788650000`), which is where that rendering comes from;
    // the others pin it on dates whose weekday is independently known: 2001-09-09
    // 01:46:40 UTC (epoch second 1000000000) was a Sunday, 2000-01-01 a Saturday and
    // 1999-12-31 a Friday.
    let cases = [
        Case {
            case: "inverted, epoch seconds",
            start: "1788660000",
            end: "1788650000",
            message: "Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time \
                      Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time",
        },
        Case {
            case: "inverted, date-time",
            start: "2026-09-06T02%3A00%3A00Z",
            end: "2026-09-05T23%3A13%3A20Z",
            message: "Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time \
                      Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time",
        },
        Case {
            case: "equal bounds span no time",
            start: "1000000000",
            end: "1000000000",
            message: "Invalid start time Sun Sep 09 01:46:40 UTC 2001 and end time \
                      Sun Sep 09 01:46:40 UTC 2001 provided: start time must be before end time",
        },
        Case {
            case: "inverted across a year boundary",
            start: "946684800",
            end: "946598400",
            message: "Invalid start time Sat Jan 01 00:00:00 UTC 2000 and end time \
                      Fri Dec 31 00:00:00 UTC 1999 provided: start time must be before end time",
        },
    ];

    let router = router();
    let entity = service_entity("checkout");
    for refused in &cases {
        for read in reads(&entity) {
            let case = format!("{} with {}", read.operation, refused.case);
            let uri = format!(
                "{}?StartTime={}&EndTime={}",
                read.path, refused.start, refused.end
            );
            let (status, answer) = call(&router, "POST", &uri, Some(read.body.clone())).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
            );
            assert_eq!(
                answer["message"].as_str(),
                Some(refused.message),
                "case {case:?}: expected the ordering refusal, got {:?}",
                answer["message"]
            );
            assert!(
                answer.get("StartTime").is_none() && answer.get("EndTime").is_none(),
                "case {case:?}: a refusal carries no window, got {answer:?}"
            );
        }
    }
}

/// The topology reads report the entities, operations and dependency edges the stored SLOs
/// monitor, and report them consistently with each other — a dependency seen from the
/// calling service is the same edge the called service sees as a dependent.
///
/// This is the round trip: the SLOs are created over HTTP and every fact the four reads
/// report is read back out of that stored state.
#[tokio::test]
async fn topology_reads_report_what_the_stored_slos_monitor() {
    let router = router();
    let now = now_seconds();

    create_slo(
        &router,
        "checkout latency",
        json!({
            "Name": "checkout-latency",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": service_entity("checkout"),
                    "OperationName": "POST /checkout",
                    "MetricType": "LATENCY",
                    "MetricDataQueries": [metric_query("m1", "AWS/ApplicationSignals", "Latency")]
                }
            }
        }),
    )
    .await;
    create_slo(
        &router,
        "cart availability",
        json!({
            "Name": "cart-availability",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": service_entity("checkout"),
                    "OperationName": "GET /cart",
                    "MetricType": "AVAILABILITY",
                    "MetricDataQueries": [metric_query("m1", "AWS/ApplicationSignals", "Fault")]
                }
            }
        }),
    )
    .await;
    create_slo(
        &router,
        "checkout calls payments",
        json!({
            "Name": "checkout-payments",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": service_entity("checkout"),
                    "OperationName": "POST /checkout",
                    "MetricType": "AVAILABILITY",
                    "DependencyConfig": {
                        "DependencyKeyAttributes": service_entity("payments"),
                        "DependencyOperationName": "Charge"
                    },
                    "MetricDataQueries": [metric_query("m1", "AWS/ApplicationSignals", "Fault")]
                }
            }
        }),
    )
    .await;
    create_slo(
        &router,
        "orders latency",
        json!({
            "Name": "orders-latency",
            "SliConfig": {
                "SliMetricConfig": {
                    "KeyAttributes": service_entity("orders"),
                    "MetricType": "LATENCY",
                    "MetricDataQueries": [metric_query("m1", "AWS/Lambda", "Duration")]
                }
            }
        }),
    )
    .await;

    // A window that contains the instant the SLOs were created, and one that ends before
    // it: an SLO is evidence of its entity from its CreatedTime onwards, so the earlier
    // window sees nothing.
    let covering = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    let earlier = format!("StartTime={}&EndTime={}", now - 7_200, now - 3_600);

    // `GetService` reports the entity the request identified, with the fullest attribute
    // set the store holds for it.
    let (status, service) = call(
        &router,
        "POST",
        &format!("/service?{covering}"),
        Some(json!({ "KeyAttributes": service_entity("checkout") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetService checkout\": expected HTTP 200, got {status} with {service:?}"
    );
    assert_eq!(
        service["Service"]["KeyAttributes"],
        service_entity("checkout"),
        "case \"GetService checkout\": expected the entity's key attributes, got {:?}",
        service["Service"]
    );
    assert_eq!(
        metric_set(&service["Service"]["MetricReferences"]),
        BTreeSet::new(),
        "case \"GetService checkout\": expected no metric reference, got {:?}",
        service["Service"]["MetricReferences"]
    );

    // An entity named by only SOME of its attributes is reported with the full set the
    // stored SLO holds for it: the attributes identify the entity, and the fuller set comes
    // out of the store, not out of the request.
    let (_, partial) = call(
        &router,
        "POST",
        &format!("/service?{covering}"),
        Some(json!({ "KeyAttributes": { "Type": "Service", "Name": "checkout" } })),
    )
    .await;
    assert_eq!(
        partial["Service"]["KeyAttributes"],
        service_entity("checkout"),
        "case \"GetService checkout by a partial identity\": expected the stored attributes, got {:?}",
        partial["Service"]["KeyAttributes"]
    );

    // Over a window in which nothing was observed, the same partial identity is reported
    // back as it was named — there is no stored observation to widen it with.
    let (_, partial_earlier) = call(
        &router,
        "POST",
        &format!("/service?{earlier}"),
        Some(json!({ "KeyAttributes": { "Type": "Service", "Name": "checkout" } })),
    )
    .await;
    assert_eq!(
        partial_earlier["Service"]["KeyAttributes"],
        json!({ "Type": "Service", "Name": "checkout" }),
        "case \"GetService checkout by a partial identity, earlier window\": expected the requested attributes, got {:?}",
        partial_earlier["Service"]["KeyAttributes"]
    );

    // An entity no stored SLO monitors is reported by its identity alone.
    let (status, ghost) = call(
        &router,
        "POST",
        &format!("/service?{covering}"),
        Some(json!({ "KeyAttributes": service_entity("ghost") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetService ghost\": expected HTTP 200, got {status} with {ghost:?}"
    );
    assert_eq!(
        ghost["Service"]["KeyAttributes"],
        service_entity("ghost"),
        "case \"GetService ghost\": expected the requested identity, got {:?}",
        ghost["Service"]
    );
    assert_eq!(
        metric_set(&ghost["Service"]["MetricReferences"]),
        BTreeSet::new(),
        "case \"GetService ghost\": expected no metric reference, got {:?}",
        ghost["Service"]["MetricReferences"]
    );

    // The operations of the entity, in name order, one entry per distinct operation the
    // stored SLOs name on it (two SLOs name `POST /checkout`, which is ONE operation).
    let (status, operations) = call(
        &router,
        "POST",
        &format!("/service-operations?{covering}"),
        Some(json!({ "KeyAttributes": service_entity("checkout") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ListServiceOperations checkout\": expected HTTP 200, got {status} with {operations:?}"
    );
    assert_eq!(
        operations["ServiceOperations"],
        json!([
            { "Name": "GET /cart", "MetricReferences": [] },
            { "Name": "POST /checkout", "MetricReferences": [] },
        ]),
        "case \"ListServiceOperations checkout\": expected both monitored operations once each, in name order, got {:?}",
        operations["ServiceOperations"]
    );

    // Another entity's operations are its own: the `orders` SLO names none.
    let (_, orders_operations) = call(
        &router,
        "POST",
        &format!("/service-operations?{covering}"),
        Some(json!({ "KeyAttributes": service_entity("orders") })),
    )
    .await;
    assert_eq!(
        orders_operations["ServiceOperations"].as_array().map(Vec::len),
        Some(0),
        "case \"ListServiceOperations orders\": expected no operation, got {:?}",
        orders_operations["ServiceOperations"]
    );

    // The dependency edge, from the calling service's end.
    let (status, dependencies) = call(
        &router,
        "POST",
        &format!("/service-dependencies?{covering}"),
        Some(json!({ "KeyAttributes": service_entity("checkout") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ListServiceDependencies checkout\": expected HTTP 200, got {status} with {dependencies:?}"
    );
    assert_eq!(
        dependencies["ServiceDependencies"],
        json!([{
            "OperationName": "POST /checkout",
            "DependencyKeyAttributes": service_entity("payments"),
            "DependencyOperationName": "Charge",
            "MetricReferences": []
        }]),
        "case \"ListServiceDependencies checkout\": expected the one stored edge, got {:?}",
        dependencies["ServiceDependencies"]
    );

    // The SAME edge, from the called service's end.
    let (status, dependents) = call(
        &router,
        "POST",
        &format!("/service-dependents?{covering}"),
        Some(json!({ "KeyAttributes": service_entity("payments") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ListServiceDependents payments\": expected HTTP 200, got {status} with {dependents:?}"
    );
    assert_eq!(
        dependents["ServiceDependents"][0]["DependentKeyAttributes"],
        service_entity("checkout"),
        "case \"ListServiceDependents payments\": expected checkout as the dependent, got {:?}",
        dependents["ServiceDependents"]
    );
    assert_eq!(
        dependents["ServiceDependents"][0]["OperationName"].as_str(),
        Some("Charge"),
        "case \"ListServiceDependents payments\": expected the invoked operation, got {:?}",
        dependents["ServiceDependents"][0]
    );
    assert_eq!(
        dependents["ServiceDependents"][0]["DependentOperationName"].as_str(),
        Some("POST /checkout"),
        "case \"ListServiceDependents payments\": expected the invoking operation, got {:?}",
        dependents["ServiceDependents"][0]
    );
    // The service the edge points at has no dependency of its own.
    let (_, none) = call(
        &router,
        "POST",
        &format!("/service-dependencies?{covering}"),
        Some(json!({ "KeyAttributes": service_entity("payments") })),
    )
    .await;
    assert_eq!(
        none["ServiceDependencies"].as_array().map(Vec::len),
        Some(0),
        "case \"ListServiceDependencies payments\": expected no edge, got {:?}",
        none["ServiceDependencies"]
    );

    // A window that ends before the SLOs existed observes none of them.
    for read in reads(&service_entity("checkout")) {
        let case = format!("{} over a window that ends before the SLOs existed", read.operation);
        let (status, answer) = call(
            &router,
            "POST",
            &format!("{}?{earlier}", read.path),
            Some(read.body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        match read.list_member {
            Some(member) => assert_eq!(
                answer[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected an empty {member}, got {:?}",
                answer[member]
            ),
            None => assert_eq!(
                metric_set(&answer["Service"]["MetricReferences"]),
                BTreeSet::new(),
                "case {case:?}: expected no metric reference, got {:?}",
                answer["Service"]["MetricReferences"]
            ),
        }
    }

    // A window that has not begun yet observes nothing either: the period an SLO is
    // evidence for ends at now.
    let ahead = format!("StartTime={}&EndTime={}", now + 3_600, now + 7_200);
    for read in reads(&service_entity("checkout")) {
        let case = format!("{} over a window that has not begun", read.operation);
        let (status, answer) = call(
            &router,
            "POST",
            &format!("{}?{ahead}", read.path),
            Some(read.body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        match read.list_member {
            Some(member) => assert_eq!(
                answer[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected an empty {member}, got {:?}",
                answer[member]
            ),
            None => assert_eq!(
                metric_set(&answer["Service"]["MetricReferences"]),
                BTreeSet::new(),
                "case {case:?}: expected no metric reference, got {:?}",
                answer["Service"]["MetricReferences"]
            ),
        }
    }
}

/// A page carries at most `MaxResults` entries, the cursor walks every entry exactly once,
/// and a token this service did not issue for that projection is refused.
#[tokio::test]
async fn pages_are_bounded_and_the_cursor_walks_every_entry_once() {
    let router = router();
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    // Five operations of one service, so a page of two leaves three pages to walk.
    let operations = ["op-1", "op-2", "op-3", "op-4", "op-5"];
    for operation in &operations {
        create_slo(
            &router,
            operation,
            json!({
                "Name": format!("slo-{operation}"),
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity("checkout"),
                        "OperationName": operation,
                        "MetricType": "LATENCY"
                    }
                }
            }),
        )
        .await;
    }

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..operations.len() + 1 {
        let case = format!("page {page}");
        let uri = match token.as_deref() {
            Some(token) => format!("/service-operations?{window}&MaxResults=2&NextToken={token}"),
            None => format!("/service-operations?{window}&MaxResults=2"),
        };
        let (status, answer) = call(
            &router,
            "POST",
            &uri,
            Some(json!({ "KeyAttributes": service_entity("checkout") })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        let items = answer["ServiceOperations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            items.len() <= 2,
            "case {case:?}: expected at most MaxResults=2 entries, got {}",
            items.len()
        );
        for item in &items {
            walked.push(item["Name"].as_str().unwrap_or_default().to_string());
        }
        token = answer["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, operations,
        "case \"paged walk\": expected every operation exactly once in name order, got {walked:?}"
    );
    assert!(
        token.is_none(),
        "case \"paged walk\": expected the last page to carry no NextToken, got {token:?}"
    );

    // A token this service never issued is refused rather than read as a boundary, and so
    // is one issued for a DIFFERENT projection: the offsets they count are not the same.
    let (_, first_page) = call(
        &router,
        "POST",
        &format!("/service-operations?{window}&MaxResults=2"),
        Some(json!({ "KeyAttributes": service_entity("checkout") })),
    )
    .await;
    let operations_token = first_page["NextToken"]
        .as_str()
        .expect("the first page of five entries carries a NextToken")
        .to_string();
    for (case, path, token) in [
        (
            "forged token",
            "/service-operations",
            "not-a-token".to_string(),
        ),
        (
            "another projection's token",
            "/service-dependencies",
            operations_token,
        ),
    ] {
        let (status, answer) = call(
            &router,
            "POST",
            &format!("{path}?{window}&MaxResults=2&NextToken={token}"),
            Some(json!({ "KeyAttributes": service_entity("checkout") })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
        assert!(
            answer["message"]
                .as_str()
                .is_some_and(|message| message.contains("NextToken")),
            "case {case:?}: expected the refusal to name NextToken, got {:?}",
            answer["message"]
        );
    }
}
