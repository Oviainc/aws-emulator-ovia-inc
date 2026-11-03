// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListServiceStates` over HTTP.
//!
//! The operation binds every input to the JSON BODY of `POST /service/states`, so each test
//! here drives the real `restJson1` surface the router production mounts: the body is the
//! bytes a client puts on the wire, and the status codes and JSON bodies are the ones a
//! client sees.
//!
//! The rule under test: the operation reports one `ServiceState` per DISCOVERED service that
//! the store records a change for INSIDE the requested period, carrying the single most
//! recent such change — so a service the store records no change for in that period has no
//! state, the reported instant is one `GetServiceLevelObjective` reports for the same stored
//! record, and an `AttributeFilter` narrows the set to the services whose attributes it
//! accepts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification
/// is off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN this emulator mints for these requests
/// carries — and therefore the Region every reported change event names, since an event
/// reports the Region of the record it was read off.
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

/// One `ListServiceStates` call.
async fn list_states(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/service/states", Some(body)).await
}

/// Seconds since the epoch, for a window around the instant the stored SLOs are written at.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
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

/// The service identities a `ListServiceStates` answer reported, as a set.
fn reported_services(answer: &Value) -> BTreeSet<String> {
    answer["ServiceStates"]
        .as_array()
        .map(|states| states.iter().map(|state| identity_key(&state["Service"])).collect())
        .unwrap_or_default()
}

/// One SLO to store: the discovered service it is evidence of, the identity its indicator
/// monitors, and the dependency it names.
struct Stored {
    case: &'static str,
    name: &'static str,
    /// The label of the discovered service in [`discovered_services`] this SLO records a
    /// change for.
    service: &'static str,
    key_attributes: Value,
    operation_name: Option<&'static str>,
    dependency: Option<Value>,
}

/// The SLOs every test here stores.
///
/// The rows differ along what the rule generalises over: the NUMBER of attributes in an
/// identity (one, two, three and the modelled maximum of four — `ServiceState.Service` is
/// `length 1..4`), the KIND of entity (`Service`, `Resource` and `AWS::Service`, `Type`
/// values the member's own doc names), the COUNT of SLOs recording a change for one service
/// (three of them name `checkout`), the ENVIRONMENT the service runs in, whether the identity
/// is a SUBSET of another observed one, and whether the SLO names an operation or a
/// dependency.
fn stored_slos() -> Vec<Stored> {
    vec![
        Stored {
            case: "a three-attribute service, with an operation",
            name: "checkout-latency",
            service: "checkout",
            key_attributes: json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" }),
            operation_name: Some("POST /checkout"),
            dependency: None,
        },
        Stored {
            case: "a second SLO on the SAME service",
            name: "checkout-availability",
            service: "checkout",
            key_attributes: json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" }),
            operation_name: Some("GET /cart"),
            dependency: None,
        },
        Stored {
            case: "an SLO naming that service by only part of its identity",
            name: "checkout-partial",
            service: "checkout",
            key_attributes: json!({ "Type": "Service", "Name": "checkout" }),
            operation_name: None,
            dependency: None,
        },
        Stored {
            case: "a service in another environment, with a dependency edge",
            name: "orders-payments",
            service: "orders",
            key_attributes: json!({
                "Type": "Service", "Name": "orders", "Environment": "staging"
            }),
            operation_name: Some("POST /orders"),
            dependency: Some(json!({
                "DependencyKeyAttributes": {
                    "Type": "RemoteService", "Name": "payments", "Environment": "staging"
                },
                "DependencyOperationName": "Charge"
            })),
        },
        Stored {
            case: "a four-attribute resource entity",
            name: "orders-table-latency",
            service: "orders-table",
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
            case: "a one-attribute identity: the entity type alone",
            name: "billing-latency",
            service: "billing",
            key_attributes: json!({ "Type": "AWS::Service" }),
            operation_name: None,
            dependency: None,
        },
    ]
}

/// The services the stored SLOs make discovered, by label: every monitored identity,
/// deduplicated, with the subset identity folded into the fuller one it names, and WITHOUT
/// `payments`, which is observed only as the far end of a dependency edge.
fn discovered_services() -> Vec<(&'static str, Value)> {
    vec![
        (
            "checkout",
            json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" }),
        ),
        (
            "orders",
            json!({ "Type": "Service", "Name": "orders", "Environment": "staging" }),
        ),
        (
            "orders-table",
            json!({
                "Type": "Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "prod"
            }),
        ),
        ("billing", json!({ "Type": "AWS::Service" })),
    ]
}

/// Store every row of [`stored_slos`] on `router`, and answer the latest `LastUpdatedTime`
/// the store holds for each discovered service — read off the records the creates answered
/// with, so the expected instant is the service's own rather than a time the test invents.
async fn seed(router: &MultiServiceRouter) -> BTreeMap<&'static str, f64> {
    let mut latest: BTreeMap<&'static str, f64> = BTreeMap::new();
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
        let (status, created) = call(
            router,
            "POST",
            "/slo",
            Some(json!({ "Name": slo.name, "SliConfig": { "SliMetricConfig": metric } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the SLO create to answer 200, got {status} with {created:?}",
            slo.case
        );
        let updated = created["Slo"]["LastUpdatedTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the create to answer a LastUpdatedTime, got {created:?}",
                slo.case
            )
        });
        latest
            .entry(slo.service)
            .and_modify(|known| *known = known.max(updated))
            .or_insert(updated);
    }
    latest
}

/// `ListServiceStates` reports one state per discovered service, each carrying exactly one
/// change event: the most recent change the store records for that service, stated with the
/// entity it applied to, this account, and the Region and the operation of the write it was
/// read off.
#[tokio::test]
async fn one_state_per_discovered_service_carries_its_latest_recorded_change() {
    let router = router();
    let latest = seed(&router).await;
    let now = now_seconds();
    let window = json!({ "StartTime": now - 3_600, "EndTime": now + 3_600 });

    let (status, answer) = list_states(&router, window.clone()).await;
    assert_eq!(
        status, 200,
        "case \"the covering window\": expected HTTP 200, got {status} with {answer:?}"
    );

    let expected: BTreeSet<String> = discovered_services()
        .iter()
        .map(|(_, identity)| identity_key(identity))
        .collect();
    assert_eq!(
        reported_services(&answer),
        expected,
        "case \"the covering window\": expected one state per discovered service, got {:?}",
        answer["ServiceStates"]
    );
    assert_eq!(
        answer["ServiceStates"].as_array().map(Vec::len),
        Some(expected.len()),
        "case \"the covering window\": expected each service exactly once, got {:?}",
        answer["ServiceStates"]
    );
    assert_eq!(
        answer["StartTime"].as_f64(),
        Some((now - 3_600) as f64),
        "case \"the covering window\": expected the requested StartTime back, got {:?}",
        answer["StartTime"]
    );
    assert_eq!(
        answer["EndTime"].as_f64(),
        Some((now + 3_600) as f64),
        "case \"the covering window\": expected the requested EndTime back, got {:?}",
        answer["EndTime"]
    );
    assert!(
        answer.get("NextToken").is_none(),
        "case \"the covering window\": a single page carries no NextToken, got {:?}",
        answer["NextToken"]
    );

    for (label, identity) in discovered_services() {
        let state = answer["ServiceStates"]
            .as_array()
            .and_then(|states| {
                states
                    .iter()
                    .find(|state| identity_key(&state["Service"]) == identity_key(&identity))
            })
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: expected a state for {identity:?}, got {:?}",
                    answer["ServiceStates"]
                )
            });

        // `LatestChangeEvents` is `@required` with `length 1..1`: one event, the latest.
        let events = state["LatestChangeEvents"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            events.len(),
            1,
            "case {label:?}: expected exactly one change event, got {:?}",
            state["LatestChangeEvents"]
        );
        let event = &events[0];
        let expected_at = latest[label];
        assert_eq!(
            event["Timestamp"].as_f64(),
            Some(expected_at),
            "case {label:?}: expected the latest recorded change at {expected_at}, got {:?}",
            event["Timestamp"]
        );
        assert_eq!(
            event["Entity"], state["Service"],
            "case {label:?}: expected the event to name the service it changed, got {:?}",
            event["Entity"]
        );
        assert_eq!(
            event["AccountId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {label:?}: expected this account, got {:?}",
            event["AccountId"]
        );
        assert_eq!(
            event["Region"].as_str(),
            Some(REGION),
            "case {label:?}: expected the Region of the record the change was read off, got {:?}",
            event["Region"]
        );
        // A configuration change: a deployment is an event of the customer's own pipeline,
        // which this emulator never observes.
        assert_eq!(
            event["ChangeEventType"].as_str(),
            Some("CONFIGURATION"),
            "case {label:?}: expected a CONFIGURATION change, got {:?}",
            event["ChangeEventType"]
        );
        // `EventId`'s own doc: "For other events, this will be `Unknown`".
        assert_eq!(
            event["EventId"].as_str(),
            Some("Unknown"),
            "case {label:?}: expected the documented EventId for a non-CloudTrail event, got {:?}",
            event["EventId"]
        );
        assert!(
            event.get("UserName").is_none(),
            "case {label:?}: expected no initiator to be invented, got {event:?}"
        );
        // Every seeded record was written by a create and never updated, so the change each
        // state reports is that create: `EventName` names the operation that wrote the
        // instant, exactly as `ListEntityEvents` names it for the same record.
        assert_eq!(
            event["EventName"].as_str(),
            Some("CreateServiceLevelObjective"),
            "case {label:?}: expected the operation that wrote the reported instant, got {:?}",
            event["EventName"]
        );
        assert!(
            state.get("AttributeFilters").is_none(),
            "case {label:?}: a request that named no filters has none applied, got {:?}",
            state["AttributeFilters"]
        );
    }

    // The order is deterministic: the same request answers in the same order, which is what
    // makes the page cursor a stable boundary.
    let (_, again) = list_states(&router, window).await;
    assert_eq!(
        again["ServiceStates"], answer["ServiceStates"],
        "case \"twice over one window\": expected the same answer both times, got {:?} then {:?}",
        answer["ServiceStates"], again["ServiceStates"]
    );
}

/// The reported instant is the one the store holds: the change event's `Timestamp` equals the
/// `LastUpdatedTime` a `GetServiceLevelObjective` answers for the SLO that recorded it, and
/// an update to that SLO moves both together.
#[tokio::test]
async fn the_reported_instant_round_trips_through_get_service_level_objective() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = json!({ "StartTime": now - 3_600, "EndTime": now + 3_600 });

    // `billing-latency` is the only SLO recording a change for its service, so that service's
    // reported instant is exactly that record's.
    for case in ["before the update", "after the update"] {
        if case == "after the update" {
            let (status, patched) = call(
                &router,
                "PATCH",
                "/slo/billing-latency",
                Some(json!({ "Id": "billing-latency", "Goal": { "AttainmentGoal": 95.0 } })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to answer 200, got {status} with {patched:?}"
            );
        }

        let (status, fetched) = call(&router, "GET", "/slo/billing-latency", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the Get to answer 200, got {status} with {fetched:?}"
        );
        let stored = fetched["Slo"]["LastUpdatedTime"].as_f64();

        let (status, answer) = list_states(&router, window.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        let reported = answer["ServiceStates"]
            .as_array()
            .and_then(|states| {
                states
                    .iter()
                    .find(|state| identity_key(&state["Service"]) == "Type=AWS::Service")
            })
            .map(|state| state["LatestChangeEvents"][0]["Timestamp"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            reported.as_f64(),
            stored,
            "case {case:?}: expected the stored LastUpdatedTime {stored:?}, got {reported:?}"
        );
    }
}

/// The window selects the changes reported: a service the store records no change for inside
/// it has no state at all, since a state carries exactly one change event.
///
/// Every bound of a window stated relative to the stored history is measured from an instant
/// the STORE recorded — the latest `LastUpdatedTime` the writes themselves answered with,
/// which [`seed`] reads back — rather than from a clock sample read before those writes: a
/// record's instant is the clock's at the moment of the write, so a sample taken earlier can
/// precede it, and a bound a second past such a sample can then still sit INSIDE the history
/// it was meant to be after. Only the empty-store row, which has no record to measure from,
/// names the clock.
///
/// The rows differ along where the window sits relative to the history (before it, after it,
/// over it, beginning just past its end), how wide it is (an hour, two, a day, a single
/// second) and whether the store holds anything at all.
#[tokio::test]
async fn the_window_selects_the_changes_reported() {
    let router = router();
    let discovered = discovered_services().len();

    // Nothing is stored yet, so there is no recorded instant to measure from and no change
    // for any window to report.
    let empty_at = now_seconds() as f64;
    let (status, answer) = list_states(
        &router,
        json!({ "StartTime": empty_at - 3_600.0, "EndTime": empty_at + 3_600.0 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an empty store over a covering window\": expected HTTP 200, got {status} with {answer:?}"
    );
    assert_eq!(
        answer["ServiceStates"].as_array().map(Vec::len),
        Some(0),
        "case \"an empty store over a covering window\": expected 0 states, got {:?}",
        answer["ServiceStates"]
    );

    // The last instant the store recorded for any service: every bound below is an offset
    // from it, so no row depends on how long the writes took.
    let last_write = seed(&router)
        .await
        .values()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        last_write.is_finite(),
        "case \"the seeded history\": expected the writes to report their instants, got {last_write}"
    );

    let cases = [
        (
            "a window that ends before anything was written",
            last_write - 7_200.0,
            last_write - 3_600.0,
            0,
        ),
        (
            "a window entirely after the writes",
            last_write + 3_600.0,
            last_write + 7_200.0,
            0,
        ),
        (
            "a window that contains the writes",
            last_write - 3_600.0,
            last_write + 3_600.0,
            discovered,
        ),
        (
            "a window that begins long ago",
            last_write - 86_400.0,
            last_write + 86_400.0,
            discovered,
        ),
        (
            "a window that begins a second after the last write",
            last_write + 1.0,
            last_write + 7_200.0,
            0,
        ),
        (
            "a one-second window just past the last write",
            last_write + 1.0,
            last_write + 2.0,
            0,
        ),
    ];

    for (case, start, end, expected) in cases {
        let (status, answer) =
            list_states(&router, json!({ "StartTime": start, "EndTime": end })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["ServiceStates"].as_array().map(Vec::len),
            Some(expected),
            "case {case:?}: expected {expected} states, got {:?}",
            answer["ServiceStates"]
        );
    }
}

/// An `AttributeFilter` narrows the reported services to those carrying the named attribute
/// with one of the named values, filters narrow together, and every reported state echoes the
/// filters that were applied.
#[tokio::test]
async fn attribute_filters_narrow_the_services_and_are_echoed() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filters at all",
            json!([]),
            vec!["checkout", "orders", "orders-table", "billing"],
        ),
        (
            "one value of one attribute",
            json!([{ "AttributeFilterName": "Name", "AttributeFilterValues": ["checkout"] }]),
            vec!["checkout"],
        ),
        (
            "an attribute two services share",
            json!([{ "AttributeFilterName": "Environment", "AttributeFilterValues": ["prod"] }]),
            vec!["checkout", "orders-table"],
        ),
        (
            "two values of one attribute match either",
            json!([{
                "AttributeFilterName": "Environment",
                "AttributeFilterValues": ["prod", "staging"]
            }]),
            vec!["checkout", "orders", "orders-table"],
        ),
        (
            "the entity type",
            json!([{ "AttributeFilterName": "Type", "AttributeFilterValues": ["Service"] }]),
            vec!["checkout", "orders"],
        ),
        (
            "two filters narrow together",
            json!([
                { "AttributeFilterName": "Type", "AttributeFilterValues": ["Service"] },
                { "AttributeFilterName": "Environment", "AttributeFilterValues": ["staging"] }
            ]),
            vec!["orders"],
        ),
        (
            "an attribute only a resource identity carries",
            json!([{
                "AttributeFilterName": "Identifier",
                "AttributeFilterValues": ["orders-table"]
            }]),
            vec!["orders-table"],
        ),
        (
            "a value no discovered service carries",
            json!([{ "AttributeFilterName": "Name", "AttributeFilterValues": ["payments"] }]),
            vec![],
        ),
        (
            "an attribute no discovered service carries",
            json!([{ "AttributeFilterName": "Platform", "AttributeFilterValues": ["EKS"] }]),
            vec![],
        ),
        (
            "a filter that accepts no value",
            json!([{ "AttributeFilterName": "Name", "AttributeFilterValues": [] }]),
            vec![],
        ),
        (
            "two filters that cannot both hold",
            json!([
                { "AttributeFilterName": "Environment", "AttributeFilterValues": ["prod"] },
                { "AttributeFilterName": "Environment", "AttributeFilterValues": ["staging"] }
            ]),
            vec![],
        ),
    ];

    let by_label: BTreeMap<&str, Value> = discovered_services().into_iter().collect();
    for (case, filters, expected_labels) in cases {
        let (status, answer) = list_states(
            &router,
            json!({
                "StartTime": now - 3_600,
                "EndTime": now + 3_600,
                "AttributeFilters": filters
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );

        let expected: BTreeSet<String> = expected_labels
            .iter()
            .map(|label| identity_key(&by_label[label]))
            .collect();
        assert_eq!(
            reported_services(&answer),
            expected,
            "case {case:?}: expected the services the filters accept, got {:?}",
            answer["ServiceStates"]
        );

        // Each state echoes the filters that WERE applied; a request that named none has
        // none applied.
        let applied = if filters.as_array().is_some_and(|filters| filters.is_empty()) {
            Value::Null
        } else {
            filters.clone()
        };
        for state in answer["ServiceStates"].as_array().cloned().unwrap_or_default() {
            let service = identity_key(&state["Service"]);
            assert_eq!(
                state.get("AttributeFilters").cloned().unwrap_or(Value::Null),
                applied,
                "case {case:?}: expected state {service:?} to echo the applied filters, got {:?}",
                state["AttributeFilters"]
            );
        }
    }

    // A filter the model refuses is refused before the handler runs: `AttributeFilters` is
    // `length ..20`, and `AttributeFilterName` carries `pattern ^[A-Za-z0-9 :/-]+$`.
    let too_many: Vec<Value> = (0..21)
        .map(|index| json!({
            "AttributeFilterName": format!("Name{index}"),
            "AttributeFilterValues": ["checkout"]
        }))
        .collect();
    for (case, filters) in [
        ("one filter past the modelled maximum", json!(too_many)),
        (
            "a filter name the modelled pattern refuses",
            json!([{ "AttributeFilterName": "Business_Unit", "AttributeFilterValues": ["retail"] }]),
        ),
    ] {
        let (status, answer) = list_states(
            &router,
            json!({
                "StartTime": now - 3_600,
                "EndTime": now + 3_600,
                "AttributeFilters": filters
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
    }
}

/// A page carries at most `MaxResults` entries, the cursor walks every reported state exactly
/// once in the order a single page reports them, and a token this service did not issue for
/// this projection is refused.
#[tokio::test]
async fn pages_are_bounded_and_the_cursor_walks_every_state_once() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let window = json!({ "StartTime": now - 3_600, "EndTime": now + 3_600 });

    let (_, whole) = list_states(&router, window.clone()).await;
    let in_order: Vec<String> = whole["ServiceStates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|state| identity_key(&state["Service"]))
        .collect();

    // The page sizes span the modelled `range ..250` of `ListServiceStatesMaxResults`: the
    // published maximum, and sizes either side of the number of reported states.
    for page_size in [1_usize, 2, 4, 250] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..in_order.len() + 2 {
            let case = format!("MaxResults={page_size}, page {page}");
            let mut body = window.clone();
            body["MaxResults"] = json!(page_size);
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let (status, answer) = list_states(&router, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
            );
            let items = answer["ServiceStates"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries, got {}",
                items.len()
            );
            walked.extend(items.iter().map(|state| identity_key(&state["Service"])));
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
            "case \"MaxResults={page_size}\": expected every state exactly once in the single-page order, got {walked:?}"
        );
    }

    // A `MaxResults` past the modelled maximum is refused rather than coerced, so a caller
    // never reads a clamped page as the whole answer.
    let mut past_the_maximum = window.clone();
    past_the_maximum["MaxResults"] = json!(251);
    let (status, answer) = list_states(&router, past_the_maximum).await;
    assert_eq!(
        status, 400,
        "case \"MaxResults past the modelled maximum\": expected HTTP 400, got {status} with {answer:?}"
    );

    // A token this service never issued is refused rather than read as a boundary, and so is
    // one issued for a DIFFERENT projection: the offsets they count are not the same.
    let mut first = window.clone();
    first["MaxResults"] = json!(1);
    let (_, first_page) = list_states(&router, first).await;
    let own_token = first_page["NextToken"]
        .as_str()
        .expect("a one-entry page of several states carries a NextToken")
        .to_string();
    let (_, services_page) = call(
        &router,
        "GET",
        &format!(
            "/services?StartTime={}&EndTime={}&MaxResults=1",
            now - 3_600,
            now + 3_600
        ),
        None,
    )
    .await;
    let foreign_token = services_page["NextToken"]
        .as_str()
        .expect("a one-entry page of several services carries a NextToken")
        .to_string();

    for (case, token) in [
        ("a forged token", "not-a-token".to_string()),
        ("another projection's token", foreign_token),
    ] {
        let mut body = window.clone();
        body["MaxResults"] = json!(1);
        body["NextToken"] = json!(token);
        let (status, answer) = list_states(&router, body).await;
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
    let mut second = window;
    second["MaxResults"] = json!(1);
    second["NextToken"] = json!(own_token);
    let (status, second_page) = list_states(&router, second).await;
    assert_eq!(
        status, 200,
        "case \"this projection's token\": expected HTTP 200, got {status} with {second_page:?}"
    );
    assert_eq!(
        reported_services(&second_page),
        in_order.iter().skip(1).take(1).cloned().collect(),
        "case \"this projection's token\": expected the second state, got {:?}",
        second_page["ServiceStates"]
    );
}

/// The store is one account's, so the account selector decides between "this account" and
/// "nothing", and `IncludeLinkedAccounts` changes no answer: there is no source account whose
/// service states it could add.
#[tokio::test]
async fn the_account_selector_selects_this_account_or_nothing() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let discovered = discovered_services().len();
    let this_account = engine::emulator_account_id().to_string();

    let cases = [
        ("no account named", None, discovered),
        ("this account named", Some(this_account), discovered),
        ("another account named", Some("210987654321".to_string()), 0),
        // The documented example account id, which is not this emulator's.
        ("a third account named", Some("123456789012".to_string()), 0),
    ];

    for (case, account, expected) in cases {
        for linked in [false, true] {
            let case = format!("{case}, IncludeLinkedAccounts={linked}");
            let mut body = json!({
                "StartTime": now - 3_600,
                "EndTime": now + 3_600,
                "IncludeLinkedAccounts": linked
            });
            if let Some(account) = account.as_deref() {
                body["AwsAccountId"] = json!(account);
            }
            let (status, answer) = list_states(&router, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
            );
            assert_eq!(
                answer["ServiceStates"].as_array().map(Vec::len),
                Some(expected),
                "case {case:?}: expected {expected} states, got {:?}",
                answer["ServiceStates"]
            );
        }
    }

    // An account id in no form the modelled `^[0-9]{12}$` pattern admits is refused.
    for (case, account) in [
        ("too few digits", "12345"),
        ("a non-digit", "12345678901x"),
        ("an empty value", ""),
    ] {
        let (status, answer) = list_states(
            &router,
            json!({
                "StartTime": now - 3_600,
                "EndTime": now + 3_600,
                "AwsAccountId": account
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
    }
}

/// A time range that does not run forwards is refused with `ValidationException` (HTTP 400)
/// naming BOTH instants, and both bounds are `@required`.
///
/// The rows differ along the relation between the bounds (inverted, equal) and the calendar
/// date, so the stated rendering is pinned on instants whose weekday is independently known:
/// epoch second 1000000000 is Sunday 2001-09-09 01:46:40 UTC, 946684800 Saturday 2000-01-01.
#[tokio::test]
async fn a_range_that_does_not_run_forwards_is_refused() {
    let router = router();
    seed(&router).await;

    let cases = [
        (
            "inverted bounds",
            1_788_660_000_i64,
            1_788_650_000_i64,
            "Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time \
             Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time",
        ),
        (
            "equal bounds span no time",
            1_000_000_000,
            1_000_000_000,
            "Invalid start time Sun Sep 09 01:46:40 UTC 2001 and end time \
             Sun Sep 09 01:46:40 UTC 2001 provided: start time must be before end time",
        ),
        (
            "inverted across a year boundary",
            946_684_800,
            946_598_400,
            "Invalid start time Sat Jan 01 00:00:00 UTC 2000 and end time \
             Fri Dec 31 00:00:00 UTC 1999 provided: start time must be before end time",
        ),
    ];

    for (case, start, end, message) in cases {
        let (status, answer) =
            list_states(&router, json!({ "StartTime": start, "EndTime": end })).await;
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
            answer.get("StartTime").is_none() && answer.get("ServiceStates").is_none(),
            "case {case:?}: a refusal carries no window and no states, got {answer:?}"
        );
    }

    let now = now_seconds();
    for (case, body) in [
        ("no StartTime", json!({ "EndTime": now })),
        ("no EndTime", json!({ "StartTime": now })),
        ("neither bound", json!({})),
    ] {
        let (status, answer) = list_states(&router, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
        );
    }
}
