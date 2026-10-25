// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of HOW a recorded change is attributed, over the two reads that
//! report a `ChangeEvent`: `ListEntityEvents` and `ListServiceStates`.
//!
//! The rule under test: a change event reports the WRITE it was read off, so its `Region` is
//! the Region of the ARN of the record holding the reported instant ("the Amazon Web Services
//! region where this change event occurred",
//! `smithy_query(query="doc",shape="ChangeEvent",member="Region")`) and its `EventName` names
//! the operation that wrote that instant ("the name or description of this change event",
//! same shape) — neither depends on which read reports the change or on where the READING
//! caller signs, so the two reads of one recorded change agree member for member.
//!
//! Every case drives the real `restJson1` surface through the router production mounts, with
//! the SigV4 credential scope of each request chosen per case: that scope is what the Region
//! of a minted ARN comes from, so a create signed in one Region and a read signed in another
//! is exactly the input the rule generalises over. (The harness below is this file's own, as
//! in every other suite in this crate: the suites are separate modules of one test target and
//! none of them exports its helpers.)

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `EventName` of each write a stored SLO is the record of.
const SLO_CREATED: &str = "CreateServiceLevelObjective";
const SLO_UPDATED: &str = "UpdateServiceLevelObjective";

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

/// One request's SigV4 credential scope, in `region`. Signature verification is off (see
/// `set_request_authentication`), so the signature bytes are never read — the scope is what
/// routes the request to this service and names the Region it is made in.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{region}/application-signals/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Send one request, signed in `region`, and return its status and parsed JSON body
/// (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    region: &str,
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
            "https://application-signals.{region}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", authorization(region))
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

/// The identity of the service entity the SLOs below monitor.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": "prod" })
}

/// Create one SLO monitoring `entity`, signed in `region` — which is the Region the stored
/// record's ARN carries. Answers the `LastUpdatedTime` the create stored.
async fn create_slo(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    entity: &Value,
) -> f64 {
    let (status, created) = call(
        router,
        region,
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
    created["Slo"]["LastUpdatedTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a stored LastUpdatedTime, got {created:?}"))
}

/// Update one stored SLO, signed in `region`. The record is addressed by name, so an update
/// signed elsewhere than the create still finds it and leaves its ARN — and therefore the
/// Region of the write it holds — exactly as the create minted it.
async fn update_slo(router: &MultiServiceRouter, case: &str, region: &str, name: &str) -> f64 {
    let (status, patched) = call(
        router,
        region,
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
        .unwrap_or_else(|| panic!("case {case:?}: expected a stored LastUpdatedTime, got {patched:?}"))
}

/// The newest change `ListEntityEvents` reports for `entity` over the window, signed in
/// `region`: the first event of the history, since that read answers newest first.
async fn newest_entity_event(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    entity: &Value,
    window: (f64, f64),
) -> Value {
    let (status, answer) = call(
        router,
        region,
        "POST",
        "/events",
        Some(json!({ "Entity": entity, "StartTime": window.0, "EndTime": window.1 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListEntityEvents to answer 200, got {status} with {answer:?}"
    );
    answer["ChangeEvents"]
        .as_array()
        .and_then(|events| events.first().cloned())
        .unwrap_or_else(|| panic!("case {case:?}: expected at least one change event, got {answer:?}"))
}

/// The one change event `ListServiceStates` reports for the service `entity` identifies over
/// the window, signed in `region`.
async fn reported_service_state_event(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    entity: &Value,
    window: (f64, f64),
) -> Value {
    let (status, answer) = call(
        router,
        region,
        "POST",
        "/service/states",
        Some(json!({ "StartTime": window.0, "EndTime": window.1 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListServiceStates to answer 200, got {status} with {answer:?}"
    );
    let state = answer["ServiceStates"]
        .as_array()
        .and_then(|states| states.iter().find(|state| &state["Service"] == entity).cloned())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a state for {entity:?}, got {answer:?}")
        });
    let events = state["LatestChangeEvents"].as_array().cloned().unwrap_or_default();
    // `LatestChangeEvents` is `@required` with `length 1..1`: one event, the latest.
    assert_eq!(
        events.len(),
        1,
        "case {case:?}: expected exactly one change event, got {:?}",
        state["LatestChangeEvents"]
    );
    events[0].clone()
}

/// One row of the table: a record written in one Region, optionally updated from another, and
/// then read by a caller signing in a third.
struct Case {
    case: &'static str,
    /// The Region the create is signed in — the Region the record's ARN carries.
    writer: &'static str,
    /// The Region a following update is signed in, when one follows.
    updater: Option<&'static str>,
    /// The Region both reads are signed in.
    reader: &'static str,
    /// The service the SLO monitors, so each row's records are its own.
    service: &'static str,
    /// The operation whose write the reported instant is.
    expected_event: &'static str,
}

/// Both reads report one recorded change identically: the Region of the record it was written
/// into and the operation that wrote it, whatever Region the reader signs in.
///
/// The rows differ along what the rule generalises over: whether the reader's Region equals
/// the writer's, the number of distinct Regions in play (one, two, three), whether the
/// reported write is the create or a later update, and whether the writer's Region is the
/// emulator's own default one (so a row cannot pass by falling back to it).
#[tokio::test]
async fn both_reads_attribute_a_change_to_the_record_it_was_written_into() {
    let cases = [
        Case {
            case: "written and read in one region",
            writer: "us-west-2",
            updater: None,
            reader: "us-west-2",
            service: "checkout",
            expected_event: SLO_CREATED,
        },
        Case {
            case: "read from another region",
            writer: "us-west-2",
            updater: None,
            reader: "us-east-1",
            service: "orders",
            expected_event: SLO_CREATED,
        },
        Case {
            case: "updated from a second region, read from a third",
            writer: "eu-central-1",
            updater: Some("ap-southeast-2"),
            reader: "us-east-1",
            service: "billing",
            expected_event: SLO_UPDATED,
        },
        Case {
            case: "written in the emulator's default region, read elsewhere",
            writer: "us-east-1",
            updater: None,
            reader: "ap-northeast-1",
            service: "inventory",
            expected_event: SLO_CREATED,
        },
        Case {
            case: "written in a region no other row names, updated in place",
            writer: "sa-east-1",
            updater: Some("sa-east-1"),
            reader: "eu-west-3",
            service: "shipping",
            expected_event: SLO_UPDATED,
        },
    ];

    for row in cases {
        let router = router();
        let entity = service_entity(row.service);
        let name = format!("{}-latency", row.service);
        let created_at = create_slo(&router, row.case, row.writer, &name, &entity).await;
        let written_at = match row.updater {
            Some(region) => update_slo(&router, row.case, region, &name).await,
            None => created_at,
        };
        let now = now_seconds();
        let window = (now - 3_600.0, now + 3_600.0);

        let from_history = newest_entity_event(&router, row.case, row.reader, &entity, window).await;
        let from_state =
            reported_service_state_event(&router, row.case, row.reader, &entity, window).await;

        for (read, event) in [("ListEntityEvents", &from_history), ("ListServiceStates", &from_state)]
        {
            assert_eq!(
                event["Timestamp"].as_f64(),
                Some(written_at),
                "case {:?}: expected {read} to report the stored instant {written_at}, got {:?}",
                row.case,
                event["Timestamp"]
            );
            assert_eq!(
                event["Region"].as_str(),
                Some(row.writer),
                "case {:?}: expected {read} to report the record's own region, got {:?}",
                row.case,
                event["Region"]
            );
            assert_eq!(
                event["EventName"].as_str(),
                Some(row.expected_event),
                "case {:?}: expected {read} to name the operation that wrote the instant, got {:?}",
                row.case,
                event["EventName"]
            );
            assert_eq!(
                event["AccountId"].as_str(),
                Some(engine::emulator_account_id()),
                "case {:?}: expected {read} to report this account, got {:?}",
                row.case,
                event["AccountId"]
            );
            assert_eq!(
                event["Entity"], entity,
                "case {:?}: expected {read} to name the entity that changed, got {:?}",
                row.case, event["Entity"]
            );
        }

        // The two reads of ONE recorded change agree member for member: nothing an event
        // carries is read off the request reporting it.
        assert_eq!(
            from_state, from_history,
            "case {:?}: expected both reads to report one recorded change identically, got {from_history:?} then {from_state:?}",
            row.case
        );
    }
}

/// The Region reported is the Region of the RECORD holding the reported instant, not of some
/// other record of the same service: two SLOs on one service, written in different Regions,
/// make the reported Region the newest write's own.
#[tokio::test]
async fn the_region_follows_the_record_the_reported_instant_came_from() {
    let case = "two records of one service, written in different regions";
    let router = router();
    let entity = service_entity("payments");

    // `(region the record was written in, the instant it holds)`, in write order.
    let mut written: Vec<(&str, f64)> = Vec::new();
    for (region, name) in [("eu-north-1", "payments-latency"), ("ca-central-1", "payments-availability")]
    {
        let at = create_slo(&router, case, region, name, &entity).await;
        written.push((region, at));
    }

    let now = now_seconds();
    let window = (now - 3_600.0, now + 3_600.0);
    let reader = "us-east-1";
    let from_state = reported_service_state_event(&router, case, reader, &entity, window).await;
    let from_history = newest_entity_event(&router, case, reader, &entity, window).await;

    let newest = written
        .iter()
        .map(|(_, at)| *at)
        .fold(f64::NEG_INFINITY, f64::max);
    assert_eq!(
        from_state["Timestamp"].as_f64(),
        Some(newest),
        "case {case:?}: expected the newest recorded instant {newest}, got {:?}",
        from_state["Timestamp"]
    );
    // The Region of whichever record holds that instant — the set is a singleton unless two
    // writes landed on the very same instant, which the ARN tie-break then decides.
    let regions_of_newest: BTreeSet<&str> = written
        .iter()
        .filter(|(_, at)| *at == newest)
        .map(|(region, _)| *region)
        .collect();
    let reported = from_state["Region"].as_str().unwrap_or_default();
    assert!(
        regions_of_newest.contains(reported),
        "case {case:?}: expected the region of the record holding the reported instant, one of \
         {regions_of_newest:?}, got {reported:?}"
    );
    assert_ne!(
        Some(reported),
        Some(reader),
        "case {case:?}: expected a region a record was written in, not the reader's, got {reported:?}"
    );
    assert_eq!(
        from_state, from_history,
        "case {case:?}: expected both reads to report one recorded change identically, got \
         {from_history:?} then {from_state:?}"
    );
}
