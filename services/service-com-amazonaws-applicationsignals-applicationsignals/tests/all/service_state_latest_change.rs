// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of WHICH recorded change `ListServiceStates` reports as a service's
//! latest one.
//!
//! The rule under test: the one `ChangeEvent` a `ServiceState` carries is the most recent of
//! EVERY dated change record this emulator holds against that service inside the requested
//! period — the writes of its SLOs and the instrumentation configurations applied to it
//! alike, which is the same set of records the history read reports ("an array containing the
//! most recent change events for this service",
//! `smithy_query(query="doc",shape="ServiceState",member="LatestChangeEvents")`) — so the two
//! reads cannot disagree about an entity's most recent change, and a record of a change to
//! ANOTHER entity is not one of them.
//!
//! The rows differ along what the rule generalises over: the KIND of record holding the
//! latest change, the ORDER the records were written in, the NUMBER of records of each kind,
//! how fully the SLO's identity names the entity, and whether a record of a neighbouring
//! entity exists. Every case drives the real `restJson1` surface the router production
//! mounts, writing each record over that same surface, so each assertion is a round trip
//! through stored state. (The harness below is this file's own, as in every other suite in
//! this crate: the suites are separate modules of one test target and none of them exports
//! its helpers.)

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN this emulator mints for these requests
/// carries — and therefore the Region every reported event names, since an event reports the
/// Region of the record it was read off.
const REGION: &str = "eu-west-2";

/// The `EventName` of each change one of this service's records is the evidence of.
const SLO_CREATED: &str = "CreateServiceLevelObjective";
const SLO_UPDATED: &str = "UpdateServiceLevelObjective";
const INSTRUMENTATION_CREATED: &str = "CreateInstrumentationConfiguration";

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

/// Whole seconds since the epoch, for a window around the instants the records are written
/// at. WHOLE seconds, because an instant goes on the wire as seconds with a fraction and the
/// bounds have to round-trip exactly; the records' own sub-second instants fall strictly
/// inside every window named below.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as f64
}

/// Create one SLO monitoring `entity` and answer the instant the create stored.
async fn create_slo(router: &MultiServiceRouter, case: &str, name: &str, entity: &Value) -> f64 {
    let (status, created) = call(
        router,
        "POST",
        "/slo",
        Some(json!({
            "Name": name,
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": entity,
                "MetricType": "LATENCY"
            }}
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO create to answer 200, got {status} with {created:?}"
    );
    created["Slo"]["CreatedTime"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a stored CreatedTime, got {created:?}")
    })
}

/// Update one stored SLO and answer the instant the update stored.
async fn update_slo(router: &MultiServiceRouter, case: &str, name: &str) -> f64 {
    let (status, patched) = call(
        router,
        "PATCH",
        &format!("/slo/{name}"),
        Some(json!({ "Description": "revised objective" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO update to answer 200, got {status} with {patched:?}"
    );
    patched["Slo"]["LastUpdatedTime"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a stored LastUpdatedTime, got {patched:?}")
        })
}

/// Create one dynamic instrumentation configuration on `service` in `environment`, at a
/// location of its own, and answer the `CreatedAt` the store wrote.
async fn create_instrumentation(
    router: &MultiServiceRouter,
    case: &str,
    service: &str,
    environment: &str,
    line_number: u32,
) -> f64 {
    let (status, created) = call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(json!({
            "InstrumentationType": "BREAKPOINT",
            "Service": service,
            "Environment": environment,
            "SignalType": "SNAPSHOT",
            "Location": { "CodeLocation": {
                "Language": "Java",
                "ClassName": "Checkout",
                "FilePath": "src/checkout/Checkout.java",
                "LineNumber": line_number
            }},
            "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the instrumentation create to answer 200, got {status} with \
         {created:?}"
    );
    created["CreatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a stored CreatedAt, got {created:?}"))
}

/// The change events `ListEntityEvents` reports for `entity` over the window, newest first.
async fn entity_events(
    router: &MultiServiceRouter,
    case: &str,
    entity: &Value,
    window: (f64, f64),
) -> Vec<Value> {
    let (status, answer) = call(
        router,
        "POST",
        "/events",
        Some(json!({ "Entity": entity, "StartTime": window.0, "EndTime": window.1 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListEntityEvents to answer 200, got {status} with {answer:?}"
    );
    answer["ChangeEvents"].as_array().cloned().unwrap_or_default()
}

/// The `ServiceState` `ListServiceStates` reports over the window for the service whose
/// `Service` identity carries every entry of `names`, or `None` when it reports none.
async fn service_state(
    router: &MultiServiceRouter,
    case: &str,
    names: &Value,
    window: (f64, f64),
) -> Option<Value> {
    let (status, answer) = call(
        router,
        "POST",
        "/service/states",
        Some(json!({ "StartTime": window.0, "EndTime": window.1 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListServiceStates to answer 200, got {status} with {answer:?}"
    );
    let wanted = names
        .as_object()
        .expect("the identity under test is a JSON object");
    answer["ServiceStates"].as_array().and_then(|states| {
        states
            .iter()
            .find(|state| {
                wanted
                    .iter()
                    .all(|(key, value)| state["Service"].get(key) == Some(value))
            })
            .cloned()
    })
}

/// The one event a reported `ServiceState` carries, checked against the member's own
/// `length 1..1`.
fn only_event(case: &str, state: &Value) -> Value {
    let events = state["LatestChangeEvents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        events.len(),
        1,
        "case {case:?}: expected exactly one latest change event, got {:?}",
        state["LatestChangeEvents"]
    );
    events[0].clone()
}

/// The identity of a service entity, in the form the `Entity` member's doc spells.
fn service_entity(name: &str, environment: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": environment })
}

/// The kinds of write a row of the table below asks for, in the order they are made.
#[derive(Clone, Copy, Debug)]
enum Write {
    /// Create the SLO on the entity under test.
    Slo,
    /// Update that SLO.
    SloUpdate,
    /// Create an instrumentation configuration on the entity under test, at a location of
    /// its own.
    Instrumentation,
    /// Create an instrumentation configuration on a NEIGHBOURING entity: the same service
    /// name in another environment, which is another entity and therefore another service's
    /// record.
    NeighbourInstrumentation,
}

/// One row: the identity the SLO names, the writes made in order, and the change the reads
/// must agree is the latest one.
#[derive(Debug)]
struct Case {
    case: &'static str,
    /// The environment the SLO's `KeyAttributes` names, or `None` for an SLO that names the
    /// service without one — a partial identity, which still identifies the entity.
    slo_environment: Option<&'static str>,
    writes: &'static [Write],
    expected_event: &'static str,
}

/// The most recent change of a service is the latest of ALL its records, whichever kind holds
/// it, and both reads agree on it.
#[tokio::test]
async fn the_latest_change_is_the_newest_record_of_any_kind() {
    const ENVIRONMENT: &str = "prod";
    const NEIGHBOUR_ENVIRONMENT: &str = "staging";

    let cases = [
        Case {
            case: "an slo write only",
            slo_environment: Some(ENVIRONMENT),
            writes: &[Write::Slo],
            expected_event: SLO_CREATED,
        },
        Case {
            case: "an instrumentation configuration after an slo",
            slo_environment: Some(ENVIRONMENT),
            writes: &[Write::Slo, Write::Instrumentation],
            expected_event: INSTRUMENTATION_CREATED,
        },
        Case {
            case: "an slo after an instrumentation configuration",
            slo_environment: Some(ENVIRONMENT),
            writes: &[Write::Instrumentation, Write::Slo],
            expected_event: SLO_CREATED,
        },
        Case {
            case: "an slo update after an instrumentation configuration",
            slo_environment: Some(ENVIRONMENT),
            writes: &[Write::Slo, Write::Instrumentation, Write::SloUpdate],
            expected_event: SLO_UPDATED,
        },
        Case {
            case: "an instrumentation configuration after an slo update",
            slo_environment: Some(ENVIRONMENT),
            writes: &[Write::Slo, Write::SloUpdate, Write::Instrumentation],
            expected_event: INSTRUMENTATION_CREATED,
        },
        Case {
            case: "two instrumentation configurations after an slo",
            slo_environment: Some(ENVIRONMENT),
            writes: &[Write::Slo, Write::Instrumentation, Write::Instrumentation],
            expected_event: INSTRUMENTATION_CREATED,
        },
        Case {
            case: "an instrumentation configuration under an slo that names no environment",
            slo_environment: None,
            writes: &[Write::Slo, Write::Instrumentation],
            expected_event: INSTRUMENTATION_CREATED,
        },
        Case {
            case: "an instrumentation configuration on a neighbouring entity only",
            slo_environment: Some(ENVIRONMENT),
            writes: &[Write::Slo, Write::NeighbourInstrumentation],
            expected_event: SLO_CREATED,
        },
        Case {
            case: "an instrumentation configuration on this entity and on a neighbouring one",
            slo_environment: Some(ENVIRONMENT),
            writes: &[
                Write::Slo,
                Write::Instrumentation,
                Write::NeighbourInstrumentation,
            ],
            expected_event: INSTRUMENTATION_CREATED,
        },
    ];

    for row in cases {
        let router = router();
        let service = "checkout";
        let slo_name = "checkout-latency";
        let slo_identity = match row.slo_environment {
            Some(environment) => service_entity(service, environment),
            None => json!({ "Type": "Service", "Name": service }),
        };
        // The identity the reads are asked about: what the SLO named, which is what the
        // discovery read reports for the service.
        let mut latest: Option<(f64, &'static str)> = None;
        let mut location = 40u32;
        for write in row.writes {
            let (at, event) = match write {
                Write::Slo => (
                    create_slo(&router, row.case, slo_name, &slo_identity).await,
                    SLO_CREATED,
                ),
                Write::SloUpdate => (
                    update_slo(&router, row.case, slo_name).await,
                    SLO_UPDATED,
                ),
                Write::Instrumentation => {
                    location += 1;
                    (
                        create_instrumentation(&router, row.case, service, ENVIRONMENT, location)
                            .await,
                        INSTRUMENTATION_CREATED,
                    )
                }
                Write::NeighbourInstrumentation => {
                    location += 1;
                    // Another entity's record: it must not be reported as this service's
                    // change, so it is left out of the expected latest change.
                    create_instrumentation(
                        &router,
                        row.case,
                        service,
                        NEIGHBOUR_ENVIRONMENT,
                        location,
                    )
                    .await;
                    continue;
                }
            };
            if latest.is_none_or(|(previous, _)| at >= previous) {
                latest = Some((at, event));
            }
        }
        let (expected_at, expected_event) = latest.expect("every row makes at least one write");
        assert_eq!(
            expected_event, row.expected_event,
            "case {:?}: expected the row's own write order to make {} the newest write, got {}",
            row.case, row.expected_event, expected_event
        );

        let now = now_seconds();
        let window = (now - 3_600.0, now + 3_600.0);
        let state = service_state(&router, row.case, &slo_identity, window)
            .await
            .unwrap_or_else(|| panic!("case {:?}: expected a state for the service", row.case));
        let reported = only_event(row.case, &state);

        assert_eq!(
            reported["EventName"].as_str(),
            Some(row.expected_event),
            "case {:?}: expected the newest recorded change to be {:?}, got {:?}",
            row.case,
            row.expected_event,
            reported["EventName"]
        );
        assert_eq!(
            reported["Timestamp"].as_f64(),
            Some(expected_at),
            "case {:?}: expected the instant {expected_at} the newest record holds, got {:?}",
            row.case,
            reported["Timestamp"]
        );
        assert_eq!(
            reported["Region"].as_str(),
            Some(REGION),
            "case {:?}: expected the region the record was written in, got {:?}",
            row.case,
            reported["Region"]
        );
        assert_eq!(
            reported["Entity"], state["Service"],
            "case {:?}: expected the event to name the service it is reported for, got {:?}",
            row.case, reported["Entity"]
        );

        // The history read, over the same window and the same identity, reports the same
        // change as its newest: neither read can hold a record the other drops.
        let history = entity_events(&router, row.case, &slo_identity, window).await;
        let newest = history.first().cloned().unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the history read to report at least one event",
                row.case
            )
        });
        for member in [
            "Timestamp",
            "EventName",
            "Region",
            "AccountId",
            "ChangeEventType",
            "EventId",
        ] {
            assert_eq!(
                reported[member], newest[member],
                "case {:?}: expected both reads to agree on {member} of the newest change, got \
                 {:?} and {:?}",
                row.case, reported[member], newest[member]
            );
        }
    }
}

/// The window still bounds the set: a period holding none of the records leaves the service
/// with no state at all, since `LatestChangeEvents` is `length 1..1` — and the history read
/// reports nothing over it either.
#[tokio::test]
async fn a_window_holding_no_record_reports_no_state() {
    let case = "a window before every record";
    let router = router();
    let entity = service_entity("checkout", "prod");
    create_slo(&router, case, "checkout-latency", &entity).await;
    create_instrumentation(&router, case, "checkout", "prod", 41).await;

    let now = now_seconds();
    let window = (now - 7_200.0, now - 3_600.0);
    let state = service_state(&router, case, &entity, window).await;
    assert!(
        state.is_none(),
        "case {case:?}: expected no state for a window holding no record, got {state:?}"
    );
    let history = entity_events(&router, case, &entity, window).await;
    assert!(
        history.is_empty(),
        "case {case:?}: expected no change event for a window holding no record, got {history:?}"
    );
}
