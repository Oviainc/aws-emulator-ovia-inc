// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListServices` over HTTP.
//!
//! The operation binds every input to the QUERY STRING (`@httpQuery`) of `GET /services`,
//! so each test here drives the real `restJson1` surface the router production mounts: the
//! query values are the bytes a client puts on the wire, and the status codes and JSON
//! bodies are the ones a client sees.
//!
//! The rule under test: the operation reports one `ServiceSummary` per DISTINCT service
//! entity the store is evidence of over the requested window, and nothing else — so a
//! summary's `KeyAttributes` is an identity `GetService` answers for with those same
//! attributes, an entity observed only as a dependency of another is not a discovered
//! service, and an identity that is a subset of another observed identity names the same
//! service rather than a second one.

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

/// One identity as a single comparable string: its entries in key order. Used so an
/// assertion names the identities a call reported rather than the order they came in.
fn identity_key(attributes: &Value) -> String {
    attributes
        .as_object()
        .map(|entries| {
            entries
                .iter()
                .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| format!("{attributes:?}"))
}

/// The identities a `ListServices` answer reported, as a set.
fn reported_identities(answer: &Value) -> BTreeSet<String> {
    answer["ServiceSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| identity_key(&summary["KeyAttributes"]))
                .collect()
        })
        .unwrap_or_default()
}

/// One SLO to store: the identity its indicator monitors, and the dependency it names.
struct Stored {
    case: &'static str,
    name: &'static str,
    key_attributes: Value,
    operation_name: Option<&'static str>,
    dependency: Option<Value>,
}

/// The SLOs every discovery test stores.
///
/// The rows differ along what the rule generalises over: the NUMBER of attributes in an
/// identity (one, two, three and the modelled maximum of four — `ServiceSummary`'s
/// `KeyAttributes` is `length 1..4`), the KIND of entity (`Service`, `RemoteService` and
/// `Resource`, the `Type` values the member's own doc names), the COUNT of SLOs per
/// identity (two SLOs on `checkout` are one service), whether the SLO names an operation,
/// and whether the identity is a SUBSET of another observed one.
fn stored_slos() -> Vec<Stored> {
    vec![
        Stored {
            case: "a three-attribute service, with an operation",
            name: "checkout-latency",
            key_attributes: json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" }),
            operation_name: Some("POST /checkout"),
            dependency: None,
        },
        Stored {
            case: "a second SLO on the SAME service",
            name: "checkout-availability",
            key_attributes: json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" }),
            operation_name: Some("GET /cart"),
            dependency: None,
        },
        Stored {
            case: "an SLO naming the same service by only part of its identity",
            name: "checkout-partial",
            key_attributes: json!({ "Type": "Service", "Name": "checkout" }),
            operation_name: None,
            dependency: None,
        },
        Stored {
            case: "a service whose only dependency edge points at another entity",
            name: "orders-payments",
            key_attributes: json!({ "Type": "Service", "Name": "orders", "Environment": "prod" }),
            operation_name: Some("POST /orders"),
            dependency: Some(json!({
                "DependencyKeyAttributes": {
                    "Type": "RemoteService", "Name": "payments", "Environment": "prod"
                },
                "DependencyOperationName": "Charge"
            })),
        },
        Stored {
            case: "a four-attribute resource entity",
            name: "orders-table-latency",
            key_attributes: json!({
                "Type": "Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "prod"
            }),
            operation_name: None,
            dependency: None,
        },
        Stored {
            case: "a one-attribute identity",
            name: "billing-latency",
            key_attributes: json!({ "Name": "billing" }),
            operation_name: None,
            dependency: None,
        },
    ]
}

/// The identities the stored SLOs make discovered services: every monitored identity,
/// deduplicated, with the subset identity folded into the fuller one it names, and WITHOUT
/// `payments`, which is observed only as the far end of a dependency edge.
fn discovered_identities() -> Vec<Value> {
    vec![
        json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" }),
        json!({ "Type": "Service", "Name": "orders", "Environment": "prod" }),
        json!({
            "Type": "Resource",
            "ResourceType": "AWS::DynamoDB::Table",
            "Identifier": "orders-table",
            "Environment": "prod"
        }),
        json!({ "Name": "billing" }),
    ]
}

/// Store every row of [`stored_slos`] on `router`.
async fn seed(router: &MultiServiceRouter) {
    for slo in stored_slos() {
        let mut metric = json!({
            "KeyAttributes": slo.key_attributes,
            "MetricType": "LATENCY"
        });
        if let Some(operation_name) = slo.operation_name {
            metric["OperationName"] = json!(operation_name);
        }
        if let Some(dependency) = slo.dependency {
            metric["DependencyConfig"] = dependency;
        }
        create_slo(
            router,
            slo.case,
            json!({ "Name": slo.name, "SliConfig": { "SliMetricConfig": metric } }),
        )
        .await;
    }
}

/// `ListServices` reports one summary per distinct discovered service, and every summary it
/// reports is an identity `GetService` answers for with the same attributes.
#[tokio::test]
async fn one_summary_per_discovered_service_round_trips_through_get_service() {
    let router = router();
    seed(&router).await;

    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    let (status, answer) = call(&router, "GET", &format!("/services?{window}"), None).await;
    assert_eq!(
        status, 200,
        "case \"ListServices over the covering window\": expected HTTP 200, got {status} with {answer:?}"
    );

    let expected: BTreeSet<String> = discovered_identities().iter().map(identity_key).collect();
    assert_eq!(
        reported_identities(&answer),
        expected,
        "case \"ListServices over the covering window\": expected one summary per discovered service, got {:?}",
        answer["ServiceSummaries"]
    );
    assert_eq!(
        answer["ServiceSummaries"].as_array().map(Vec::len),
        Some(expected.len()),
        "case \"ListServices over the covering window\": expected each service exactly once, got {:?}",
        answer["ServiceSummaries"]
    );
    assert_eq!(
        answer["StartTime"].as_f64(),
        Some((now - 3_600) as f64),
        "case \"ListServices over the covering window\": expected the requested StartTime back, got {:?}",
        answer["StartTime"]
    );
    assert_eq!(
        answer["EndTime"].as_f64(),
        Some((now + 3_600) as f64),
        "case \"ListServices over the covering window\": expected the requested EndTime back, got {:?}",
        answer["EndTime"]
    );
    assert!(
        answer.get("NextToken").is_none(),
        "case \"ListServices over the covering window\": a single page carries no NextToken, got {:?}",
        answer["NextToken"]
    );

    // The order is deterministic: the same request answers in the same order, which is what
    // makes the page cursor a stable boundary.
    let (_, again) = call(&router, "GET", &format!("/services?{window}"), None).await;
    assert_eq!(
        again["ServiceSummaries"], answer["ServiceSummaries"],
        "case \"ListServices twice\": expected the same order both times, got {:?} then {:?}",
        answer["ServiceSummaries"], again["ServiceSummaries"]
    );

    for summary in answer["ServiceSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let case = format!("summary {}", identity_key(&summary["KeyAttributes"]));
        // `MetricReferences` is `@required`, so it is PRESENT and empty — this emulator
        // receives no telemetry to learn a metric from — and the telemetry-derived members
        // stay absent rather than being invented.
        assert_eq!(
            summary["MetricReferences"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected a present, empty MetricReferences, got {:?}",
            summary["MetricReferences"]
        );
        assert!(
            summary.get("AttributeMaps").is_none() && summary.get("ServiceGroups").is_none(),
            "case {case:?}: expected no telemetry-derived members, got {summary:?}"
        );

        // The round trip: the identity the list reported is one `GetService` answers for
        // with exactly those attributes.
        let (status, service) = call(
            &router,
            "POST",
            &format!("/service?{window}"),
            Some(json!({ "KeyAttributes": summary["KeyAttributes"] })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetService to answer 200, got {status} with {service:?}"
        );
        assert_eq!(
            service["Service"]["KeyAttributes"], summary["KeyAttributes"],
            "case {case:?}: expected GetService to echo the reported identity, got {:?}",
            service["Service"]["KeyAttributes"]
        );
    }
}

/// An SLO is evidence of its service from its `CreatedTime` onwards, so the window selects
/// what was discovered: a window that ends before anything existed reports nothing, and one
/// that starts in the future reports nothing however far ahead it ends.
#[tokio::test]
async fn the_window_selects_what_was_discovered() {
    let router = router();
    let now = now_seconds();
    let discovered = discovered_identities().len();

    let cases = [
        ("an empty store over a covering window", now - 3_600, now + 3_600, 0),
        ("a window that ends before anything existed", now - 7_200, now - 3_600, 0),
        ("a window entirely in the future", now + 3_600, now + 7_200, 0),
        ("a window that contains the creations", now - 3_600, now + 3_600, discovered),
        ("a window that begins long ago", now - 86_400, now + 86_400, discovered),
    ];

    for (case, start, end, expected) in cases {
        // The first row runs before anything is stored; the rest after.
        if case == "a window that ends before anything existed" {
            seed(&router).await;
        }
        let (status, answer) = call(
            &router,
            "GET",
            &format!("/services?StartTime={start}&EndTime={end}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["ServiceSummaries"].as_array().map(Vec::len),
            Some(expected),
            "case {case:?}: expected {expected} summaries, got {:?}",
            answer["ServiceSummaries"]
        );
    }
}

/// A page carries at most `MaxResults` entries, the cursor walks every discovered service
/// exactly once in the order a single page reports them, and a token this service did not
/// issue for this projection is refused.
#[tokio::test]
async fn pages_are_bounded_and_the_cursor_walks_every_service_once() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    let (_, whole) = call(&router, "GET", &format!("/services?{window}"), None).await;
    let in_order: Vec<String> = whole["ServiceSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| identity_key(&summary["KeyAttributes"]))
        .collect();

    // The page sizes span the modelled `range 1..100` of `ListServicesMaxResults`: both
    // published bounds, and sizes either side of the number of discovered services.
    for page_size in [1_usize, 2, 3, 4, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..in_order.len() + 2 {
            let case = format!("MaxResults={page_size}, page {page}");
            let uri = match token.as_deref() {
                Some(token) => {
                    format!("/services?{window}&MaxResults={page_size}&NextToken={token}")
                }
                None => format!("/services?{window}&MaxResults={page_size}"),
            };
            let (status, answer) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
            );
            let items = answer["ServiceSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries, got {}",
                items.len()
            );
            walked.extend(
                items
                    .iter()
                    .map(|summary| identity_key(&summary["KeyAttributes"])),
            );
            token = answer["NextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case \"MaxResults={page_size}\": expected the walk to end without a NextToken, got {token:?}"
        );
        assert_eq!(
            walked, in_order,
            "case \"MaxResults={page_size}\": expected every discovered service exactly once in the single-page order, got {walked:?}"
        );
    }

    // A `MaxResults` outside the modelled range is refused rather than coerced, so a caller
    // never reads a clamped page as the whole answer.
    for (case, max_results) in [("below the minimum", "0"), ("above the maximum", "101")] {
        let (status, answer) = call(
            &router,
            "GET",
            &format!("/services?{window}&MaxResults={max_results}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for MaxResults={max_results}, got {status} with {answer:?}"
        );
    }

    // A token this service never issued is refused rather than read as a boundary, and so is
    // one issued for a DIFFERENT projection: the offsets they count are not the same.
    let (_, first_page) = call(
        &router,
        "GET",
        &format!("/services?{window}&MaxResults=1"),
        None,
    )
    .await;
    let services_token = first_page["NextToken"]
        .as_str()
        .expect("a one-entry page of several services carries a NextToken")
        .to_string();
    let (_, operations_page) = call(
        &router,
        "POST",
        &format!("/service-operations?{window}&MaxResults=1"),
        Some(json!({ "KeyAttributes": {
            "Type": "Service", "Name": "checkout", "Environment": "prod"
        }})),
    )
    .await;
    let foreign_token = operations_page["NextToken"]
        .as_str()
        .expect("a one-entry page of two monitored operations carries a NextToken")
        .to_string();

    for (case, token) in [
        ("a forged token", "not-a-token".to_string()),
        ("another projection's token", foreign_token),
    ] {
        let (status, answer) = call(
            &router,
            "GET",
            &format!("/services?{window}&MaxResults=1&NextToken={token}"),
            None,
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

    // This projection's own token IS honoured, on the same window.
    let (status, second_page) = call(
        &router,
        "GET",
        &format!("/services?{window}&MaxResults=1&NextToken={services_token}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"this projection's token\": expected HTTP 200, got {status} with {second_page:?}"
    );
    assert_eq!(
        reported_identities(&second_page),
        in_order.iter().skip(1).take(1).cloned().collect(),
        "case \"this projection's token\": expected the second service, got {:?}",
        second_page["ServiceSummaries"]
    );
}

/// The store is one account's, so the account selector decides between "this account" and
/// "nothing", and `IncludeLinkedAccounts` changes no answer: there is no source account
/// whose services it could add.
#[tokio::test]
async fn the_account_selector_selects_this_account_or_nothing() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    let discovered = discovered_identities().len();
    let this_account = engine::emulator_account_id().to_string();

    let cases = [
        ("no account named", None, discovered),
        ("this account named", Some(this_account.clone()), discovered),
        ("another account named", Some("210987654321".to_string()), 0),
        // The documented example account id, which is not this emulator's
        // (`emulator_server::EMULATOR_ACCOUNT_ID` is `000000000000`).
        ("a third account named", Some("123456789012".to_string()), 0),
    ];

    for (case, account, expected) in cases {
        for linked in [false, true] {
            let case = format!("{case}, IncludeLinkedAccounts={linked}");
            let uri = match account.as_deref() {
                Some(account) => format!(
                    "/services?{window}&IncludeLinkedAccounts={linked}&AwsAccountId={account}"
                ),
                None => format!("/services?{window}&IncludeLinkedAccounts={linked}"),
            };
            let (status, answer) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
            );
            assert_eq!(
                answer["ServiceSummaries"].as_array().map(Vec::len),
                Some(expected),
                "case {case:?}: expected {expected} summaries, got {:?}",
                answer["ServiceSummaries"]
            );
        }
    }

    // An account id in no form the modelled `^[0-9]{12}$` pattern admits is refused.
    for (case, account) in [
        ("too few digits", "12345"),
        ("a non-digit", "12345678901x"),
        ("an empty value", ""),
    ] {
        let (status, answer) = call(
            &router,
            "GET",
            &format!("/services?{window}&AwsAccountId={account}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
    }
}

/// A time range that does not run forwards is refused with `ValidationException` (HTTP 400)
/// naming BOTH instants, in whichever documented form they were sent.
///
/// The rows differ along the relation between the bounds (inverted, equal), the wire form
/// they were sent in (epoch seconds, RFC-3339 `date-time`) and the calendar date, so the
/// stated rendering is pinned on instants whose weekday is independently known: epoch
/// second 1000000000 is Sunday 2001-09-09 01:46:40 UTC, 946684800 Saturday 2000-01-01.
#[tokio::test]
async fn a_range_that_does_not_run_forwards_is_refused() {
    let cases = [
        (
            "inverted, epoch seconds",
            "1788660000",
            "1788650000",
            "Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time \
             Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time",
        ),
        (
            "inverted, date-time",
            "2026-09-06T02%3A00%3A00Z",
            "2026-09-05T23%3A13%3A20Z",
            "Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time \
             Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time",
        ),
        (
            "equal bounds span no time",
            "1000000000",
            "1000000000",
            "Invalid start time Sun Sep 09 01:46:40 UTC 2001 and end time \
             Sun Sep 09 01:46:40 UTC 2001 provided: start time must be before end time",
        ),
        (
            "inverted across a year boundary",
            "946684800",
            "946598400",
            "Invalid start time Sat Jan 01 00:00:00 UTC 2000 and end time \
             Fri Dec 31 00:00:00 UTC 1999 provided: start time must be before end time",
        ),
    ];

    let router = router();
    seed(&router).await;
    for (case, start, end, message) in cases {
        let (status, answer) = call(
            &router,
            "GET",
            &format!("/services?StartTime={start}&EndTime={end}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(message),
            "case {case:?}: expected the ordering refusal, got {:?}",
            answer["message"]
        );
        assert!(
            answer.get("StartTime").is_none() && answer.get("ServiceSummaries").is_none(),
            "case {case:?}: a refusal carries no window and no summaries, got {answer:?}"
        );
    }

    // Both members are `@required`: a call that omits one is refused rather than defaulted.
    for (case, query) in [
        ("no StartTime", format!("EndTime={}", now_seconds())),
        ("no EndTime", format!("StartTime={}", now_seconds())),
        ("neither bound", String::new()),
    ] {
        let (status, answer) = call(&router, "GET", &format!("/services?{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
    }
}
