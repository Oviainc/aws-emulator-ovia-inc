// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListEntityEvents` over HTTP.
//!
//! The rules under test:
//!
//! * the operation reports ONE change event per dated record this service holds against the
//!   entity the request's `Entity` identifies — the SLOs configured on it (their
//!   `CreatedTime`, and the `LastUpdatedTime` of an update that followed) and the
//!   instrumentation configurations applied to it (their `CreatedAt`) — most recent first,
//!   each carrying the identity the record holds rather than the one the request named;
//! * the requested period selects them, inclusive at both ends;
//! * `MaxResults`/`NextToken` page over that order, and a token this service did not issue
//!   is refused;
//! * a range that does not run forwards, and an `Entity` map that identifies no entity, are
//!   refused with the modelled `ValidationException` (HTTP 400) — the latter naming the
//!   member THIS operation carries (`entity`).
//!
//! Every case drives the real `restJson1` surface through the router production mounts, so
//! the routes, statuses and JSON bodies asserted here are the ones a client sees. The
//! records the events are read back out of are created over that same surface, so each
//! assertion is a round trip through stored state. (The harness below is this file's own, as
//! in every other suite in this crate: the suites are separate modules of one test target
//! and none of them exports its helpers.)

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
/// carries — and therefore the Region every event reports.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id this store belongs to (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The only `ChangeEventType` a record of a configuration change can carry
/// (`smithy_query(query="shape",shape="ChangeEventType")` -> `["DEPLOYMENT",
/// "CONFIGURATION"]`).
const CONFIGURATION: &str = "CONFIGURATION";

/// The `EventId` of an event that is not a CloudTrail one, which the member's own doc states:
/// "For CloudTrail-based events, this is the CloudTrail event id. For other events, this will
/// be `Unknown`" (`smithy_query(query="doc",shape="ChangeEvent",member="EventId")`).
const UNKNOWN: &str = "Unknown";

/// The `EventName` of each change a stored record is the evidence of.
const SLO_CREATED: &str = "CreateServiceLevelObjective";
const SLO_UPDATED: &str = "UpdateServiceLevelObjective";
const INSTRUMENTATION_CREATED: &str = "CreateInstrumentationConfiguration";

/// The service and environment the instrumentation configuration below is applied to.
const INSTRUMENTED_SERVICE: &str = "payment-service";
const INSTRUMENTED_ENVIRONMENT: &str = "eks:cluster-prod/namespace";

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

/// Send one request and return its status, parsed JSON body (`Value::Null` when empty) and
/// `x-amzn-errortype` header (empty when absent).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value, String) {
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
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed, errortype)
}

/// Whole seconds since the epoch, for a window around the instant the records below are
/// written at.
///
/// WHOLE seconds: an instant goes on the wire as a number of seconds with a fraction, and
/// the fraction is rendered to microseconds, so an instant carrying more precision than that
/// is not the instant that comes back. Every window below is therefore named in whole
/// seconds, which round-trips exactly — the records' own sub-second instants fall strictly
/// inside these bounds.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as f64
}

/// One Application Signals service entity's identity.
fn service_entity(name: &str, environment: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": environment })
}

/// The identity of the resource entity one of the stored SLOs monitors: the four-attribute
/// form the `Entity` member's doc spells for a resource.
fn resource_entity() -> Value {
    json!({
        "Type": "AWS::Resource",
        "ResourceType": "AWS::DynamoDB::Table",
        "Identifier": "orders-table"
    })
}

/// `POST /events`, over the window `start`..`end` with the optional page controls.
async fn list_events(
    router: &MultiServiceRouter,
    entity: &Value,
    start: f64,
    end: f64,
    query: &str,
) -> (u16, Value, String) {
    call(
        router,
        "POST",
        &format!("/events{query}"),
        Some(json!({ "Entity": entity, "StartTime": start, "EndTime": end })),
    )
    .await
}

/// Create one SLO monitoring `key_attributes` and assert it was stored.
async fn create_slo(router: &MultiServiceRouter, name: &str, key_attributes: &Value) {
    let (status, answer, _) = call(
        router,
        "POST",
        "/slo",
        Some(json!({
            "Name": name,
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": key_attributes,
                "MetricType": "LATENCY"
            }}
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the SLO create to answer 200, got {status} with {answer}"
    );
}

/// Update one stored SLO, which is what dates its `LastUpdatedTime` after its `CreatedTime`.
async fn update_slo(router: &MultiServiceRouter, name: &str) {
    let (status, answer, _) = call(
        router,
        "PATCH",
        &format!("/slo/{name}"),
        Some(json!({ "Description": "revised objective" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the SLO update to answer 200, got {status} with {answer}"
    );
}

/// Create one dynamic instrumentation configuration on [`INSTRUMENTED_SERVICE`] in
/// [`INSTRUMENTED_ENVIRONMENT`].
async fn create_instrumentation(router: &MultiServiceRouter) {
    let (status, answer, _) = call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(json!({
            "InstrumentationType": "BREAKPOINT",
            "Service": INSTRUMENTED_SERVICE,
            "Environment": INSTRUMENTED_ENVIRONMENT,
            "SignalType": "SNAPSHOT",
            "Location": { "CodeLocation": {
                "Language": "Java",
                "FilePath": "src/payment/PaymentProcessor.java",
                "LineNumber": 42
            }},
            "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"instrumentation create\": expected 200, got {status} with {answer}"
    );
}

/// A store holding one dated record per change below, written over HTTP in THIS order, so
/// the history every test reads back is fully determined:
///
/// 1. an SLO on the `checkout` service,
/// 2. a second SLO on the same service (two records on one entity),
/// 3. an SLO on the `orders` service,
/// 4. an SLO on a DynamoDB table resource (an entity of another `Type`),
/// 5. an update of the first SLO (a second event on a record that already had one),
/// 6. an instrumentation configuration on the `payment-service` service.
async fn seeded_router() -> MultiServiceRouter {
    let router = router();
    create_slo(&router, "checkout-latency", &service_entity("checkout", "prod")).await;
    create_slo(
        &router,
        "checkout-availability",
        &service_entity("checkout", "prod"),
    )
    .await;
    create_slo(&router, "orders-latency", &service_entity("orders", "prod")).await;
    create_slo(&router, "orders-table-latency", &resource_entity()).await;
    update_slo(&router, "checkout-latency").await;
    create_instrumentation(&router).await;
    router
}

/// The `ChangeEvents` of one answer, as `(EventName, Entity)` pairs in the order reported.
fn reported(answer: &Value) -> Vec<(String, Value)> {
    answer["ChangeEvents"]
        .as_array()
        .expect("ChangeEvents is a required output member")
        .iter()
        .map(|event| {
            (
                event["EventName"].as_str().unwrap_or_default().to_string(),
                event["Entity"].clone(),
            )
        })
        .collect()
}

/// Assert every fact an event carries beside the change it names: the type of change, the
/// account and Region of the record it was read from, the documented `EventId` of a
/// non-CloudTrail event, and the absence of a user this emulator never recorded.
fn assert_common_members(case: &str, events: &Value) {
    let events = events
        .as_array()
        .expect("ChangeEvents is a required output member");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(
            event["ChangeEventType"].as_str(),
            Some(CONFIGURATION),
            "case {case:?}: event {index} is a configuration change, got {:?}",
            event["ChangeEventType"]
        );
        assert_eq!(
            event["AccountId"].as_str(),
            Some(ACCOUNT),
            "case {case:?}: event {index} is in this store's account, got {:?}",
            event["AccountId"]
        );
        assert_eq!(
            event["Region"].as_str(),
            Some(REGION),
            "case {case:?}: event {index} was recorded in {REGION}, got {:?}",
            event["Region"]
        );
        assert_eq!(
            event["EventId"].as_str(),
            Some(UNKNOWN),
            "case {case:?}: event {index} is not a CloudTrail event, got {:?}",
            event["EventId"]
        );
        assert!(
            event.get("UserName").is_none(),
            "case {case:?}: event {index} names no user, got {:?}",
            event["UserName"]
        );
        assert!(
            event["Timestamp"].as_f64().is_some(),
            "case {case:?}: event {index} is dated, got {:?}",
            event["Timestamp"]
        );
    }
    // Newest first, which is also what makes the page cursor a stable boundary.
    for window in events.windows(2) {
        let (earlier, later) = (
            window[0]["Timestamp"].as_f64().unwrap_or_default(),
            window[1]["Timestamp"].as_f64().unwrap_or_default(),
        );
        assert!(
            earlier >= later,
            "case {case:?}: expected the events newest first, got {earlier} before {later}"
        );
    }
}

/// Every event reported for one entity is read out of a record the store holds against it,
/// and an entity the store holds nothing against has no history at all.
///
/// The rows differ along what the rule generalises over: the KIND of record the event comes
/// from (an SLO create, an SLO update, an instrumentation configuration create), the NUMBER
/// of records held against one entity (none, one, three), the entity `Type` (`Service`,
/// `AWS::Resource`), the SHAPE of the identity the request names (the full three-attribute
/// service identity, a resource's `ResourceType`/`Identifier` pair, a partial identity, the
/// type alone — which identifies every service entity at once) and therefore how many
/// distinct entities one answer spans.
#[tokio::test]
async fn every_event_is_read_out_of_a_record_the_store_holds_for_the_entity() {
    /// One queried entity and the history it has.
    struct Case {
        label: &'static str,
        entity: Value,
        /// `(EventName, Entity)` per event, newest first.
        expected: Vec<(&'static str, Value)>,
    }

    let checkout = service_entity("checkout", "prod");
    let orders = service_entity("orders", "prod");
    let instrumented = service_entity(INSTRUMENTED_SERVICE, INSTRUMENTED_ENVIRONMENT);

    let cases = vec![
        Case {
            label: "a service with two SLOs, one of them updated",
            entity: checkout.clone(),
            expected: vec![
                (SLO_UPDATED, checkout.clone()),
                (SLO_CREATED, checkout.clone()),
                (SLO_CREATED, checkout.clone()),
            ],
        },
        Case {
            label: "a service with one SLO",
            entity: orders.clone(),
            expected: vec![(SLO_CREATED, orders.clone())],
        },
        Case {
            label: "a resource entity, by its type and identifier",
            entity: json!({
                "Type": "AWS::Resource",
                "Identifier": "orders-table"
            }),
            expected: vec![(SLO_CREATED, resource_entity())],
        },
        Case {
            label: "a service an instrumentation configuration is applied to",
            entity: instrumented.clone(),
            expected: vec![(INSTRUMENTATION_CREATED, instrumented.clone())],
        },
        Case {
            label: "a partial identity: the full stored identity comes back",
            entity: json!({ "Type": "Service", "Name": "checkout" }),
            expected: vec![
                (SLO_UPDATED, checkout.clone()),
                (SLO_CREATED, checkout.clone()),
                (SLO_CREATED, checkout.clone()),
            ],
        },
        Case {
            label: "the entity type alone: every service entity's history at once",
            entity: json!({ "Type": "Service" }),
            expected: vec![
                (INSTRUMENTATION_CREATED, instrumented),
                (SLO_UPDATED, checkout.clone()),
                (SLO_CREATED, orders),
                (SLO_CREATED, checkout.clone()),
                (SLO_CREATED, checkout),
            ],
        },
        Case {
            label: "an entity the store holds no record against",
            entity: service_entity("ghost", "prod"),
            expected: Vec::new(),
        },
    ];

    let router = seeded_router().await;
    let now = now_seconds();
    let (start, end) = (now - 3_600.0, now + 3_600.0);

    for case in cases {
        let label = case.label;
        let (status, answer, _) = list_events(&router, &case.entity, start, end, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {answer}"
        );
        assert_eq!(
            (answer["StartTime"].as_f64(), answer["EndTime"].as_f64()),
            (Some(start), Some(end)),
            "case {label:?}: expected the requested window back, got {:?}..{:?}",
            answer["StartTime"],
            answer["EndTime"]
        );
        let expected: Vec<(String, Value)> = case
            .expected
            .iter()
            .map(|(name, entity)| ((*name).to_string(), entity.clone()))
            .collect();
        assert_eq!(
            reported(&answer),
            expected,
            "case {label:?}: expected the events of the records the store holds, got {}",
            answer["ChangeEvents"]
        );
        assert_common_members(label, &answer["ChangeEvents"]);
        // Every row fits inside the modelled `MaxResults` default of 20, so no row is a page
        // boundary.
        assert!(
            answer.get("NextToken").is_none(),
            "case {label:?}: expected a single page, got {:?}",
            answer["NextToken"]
        );
    }
}

/// The requested period selects which events are reported, over the wire.
///
/// The rows differ along where the window sits relative to the history: over it, wholly
/// before it, wholly after it, and beginning after the last change. The instants the
/// records are written at are the clock's, so the bounds here are a second or more away from
/// them; the EXACT boundary rule is asserted below, on the store the handler reads, where
/// the instants are chosen rather than observed.
#[tokio::test]
async fn the_requested_period_selects_the_events() {
    let router = seeded_router().await;
    let checkout = service_entity("checkout", "prod");
    let now = now_seconds();

    let cases: Vec<(&str, f64, f64, usize)> = vec![
        ("a window over the history", now - 3_600.0, now + 3_600.0, 3),
        ("a window beginning after the last change", now + 1.0, now + 3_600.0, 0),
        ("a window entirely in the future", now + 3_600.0, now + 7_200.0, 0),
        ("a window entirely in the past", now - 7_200.0, now - 3_600.0, 0),
    ];

    for (label, start, end, expected) in cases {
        let (status, answer, _) = list_events(&router, &checkout, start, end, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {answer}"
        );
        assert_eq!(
            answer["ChangeEvents"].as_array().map(Vec::len),
            Some(expected),
            "case {label:?}: expected {expected} event(s) in {start}..{end}, got {}",
            answer["ChangeEvents"]
        );
        assert_common_members(label, &answer["ChangeEvents"]);
    }
}

/// The period is INCLUSIVE at both ends, and an event falls in it exactly when its instant
/// does.
///
/// A record's instant is the clock's, and the wire rendering of an instant is not precise to
/// the nanosecond, so no request can name one of them exactly. This rule is therefore
/// asserted on the store the handler reads, through the same method it calls, over records
/// dated at instants the test chooses — the way this crate's other clock-exact rules are
/// pinned.
///
/// The rows differ along which bound the boundary case tests, which side of it the events
/// fall on, and how much of the history the window spans: all of it, one event at either
/// end, a single instant, and none of it.
#[test]
fn the_period_is_inclusive_at_both_ends() {
    use std::collections::HashMap;

    use aws_smithy_types::DateTime;
    use service_com_amazonaws_applicationsignals_applicationsignals::ops::types::{
        ServiceLevelIndicatorConfig, ServiceLevelIndicatorMetricConfig, ServiceLevelObjective,
    };
    use service_com_amazonaws_applicationsignals_applicationsignals::state::{
        goal_with_defaults, sli_from_config, slo_arn,
        ComAmazonawsApplicationsignalsApplicationsignalsState,
    };

    /// One stored SLO on `entity`, created at `created` and last updated at `updated`
    /// (epoch seconds), built through the same derivation the create handler uses.
    fn stored_slo(
        name: &str,
        entity: &HashMap<String, String>,
        created: f64,
        updated: f64,
    ) -> ServiceLevelObjective {
        ServiceLevelObjective {
            arn: slo_arn(REGION, name),
            name: name.to_string(),
            description: None,
            created_time: DateTime::from_secs_f64(created),
            last_updated_time: DateTime::from_secs_f64(updated),
            sli: Some(sli_from_config(&ServiceLevelIndicatorConfig {
                sli_metric_config: ServiceLevelIndicatorMetricConfig {
                    key_attributes: Some(entity.clone()),
                    ..Default::default()
                },
                ..Default::default()
            })),
            request_based_sli: None,
            evaluation_type: None,
            goal: goal_with_defaults(None),
            burn_rate_configurations: None,
            metric_source_type: None,
            auto_investigation_enabled: None,
        }
    }

    // Three changes, at three instants of the test's choosing: one SLO created at 1000 and
    // never updated, and one created at 2000 and updated at 3000.
    let entity = HashMap::from([
        ("Type".to_string(), "Service".to_string()),
        ("Name".to_string(), "checkout".to_string()),
    ]);
    let mut store = ComAmazonawsApplicationsignalsApplicationsignalsState::default();
    store
        .slos
        .insert("first".to_string(), stored_slo("first", &entity, 1_000.0, 1_000.0));
    store
        .slos
        .insert("second".to_string(), stored_slo("second", &entity, 2_000.0, 3_000.0));

    // The whole history, newest first: the update, then the second create, then the first.
    let whole = store.entity_change_events(
        &entity,
        &DateTime::from_secs_f64(0.0),
        &DateTime::from_secs_f64(10_000.0),
    );
    let instants: Vec<f64> = whole
        .iter()
        .map(|event| event.timestamp.as_secs_f64())
        .collect();
    assert_eq!(
        instants,
        vec![3_000.0, 2_000.0, 1_000.0],
        "case \"the whole history\": expected the three changes newest first, got {instants:?}"
    );

    let cases: Vec<(&str, f64, f64, Vec<f64>)> = vec![
        ("exactly the span of the history", 1_000.0, 3_000.0, vec![3_000.0, 2_000.0, 1_000.0]),
        ("beginning exactly at the oldest change", 1_000.0, 1_500.0, vec![1_000.0]),
        ("ending exactly at the oldest change", 500.0, 1_000.0, vec![1_000.0]),
        ("beginning exactly at the newest change", 3_000.0, 4_000.0, vec![3_000.0]),
        ("a single instant that is a change", 2_000.0, 2_000.0, vec![2_000.0]),
        ("a single instant that is no change", 2_500.0, 2_500.0, Vec::new()),
        ("beginning just after the oldest change", 1_000.001, 3_000.0, vec![3_000.0, 2_000.0]),
        ("ending just before the newest change", 1_000.0, 2_999.999, vec![2_000.0, 1_000.0]),
        ("entirely before the history", 0.0, 999.0, Vec::new()),
        ("entirely after the history", 3_000.001, 9_000.0, Vec::new()),
    ];

    for (label, start, end, expected) in cases {
        let reported: Vec<f64> = store
            .entity_change_events(
                &entity,
                &DateTime::from_secs_f64(start),
                &DateTime::from_secs_f64(end),
            )
            .iter()
            .map(|event| event.timestamp.as_secs_f64())
            .collect();
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the changes at {expected:?} in {start}..{end}, got \
             {reported:?}"
        );
    }
}

/// `MaxResults` and `NextToken` page over the same order a single page reports, without
/// skipping or repeating an event, and the last page carries no token.
///
/// The rows differ along the page size: one event per page, a size that does not divide the
/// history, and a size that covers all of it.
#[tokio::test]
async fn the_history_pages_without_skipping_or_repeating_an_event() {
    let router = seeded_router().await;
    // The type alone, so one history spans every service entity the store holds — five
    // events, the longest history these records make.
    let entity = json!({ "Type": "Service" });
    let now = now_seconds();
    let (start, end) = (now - 3_600.0, now + 3_600.0);

    let (_, single, _) = list_events(&router, &entity, start, end, "?MaxResults=250").await;
    let whole = reported(&single);
    assert_eq!(
        whole.len(),
        5,
        "case \"one page\": expected the five stored changes, got {single}"
    );

    for page_size in [1_usize, 2, 5] {
        let label = format!("{page_size} event(s) per page");
        let mut collected: Vec<(String, Value)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match token.as_deref() {
                None => format!("?MaxResults={page_size}"),
                Some(token) => format!("?MaxResults={page_size}&NextToken={token}"),
            };
            let (status, answer, _) = list_events(&router, &entity, start, end, &query).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200, got {status} with {answer}"
            );
            let page = reported(&answer);
            assert!(
                page.len() <= page_size && !page.is_empty(),
                "case {label:?}: expected a non-empty page of at most {page_size}, got {}",
                answer["ChangeEvents"]
            );
            assert_common_members(&label, &answer["ChangeEvents"]);
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= 6,
                "case {label:?}: the paging did not terminate, got {collected:?}"
            );
            match answer["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, whole,
            "case {label:?}: expected the pages to rebuild the whole history in order"
        );
        assert_eq!(
            pages,
            whole.len().div_ceil(page_size),
            "case {label:?}: expected {} page(s) for {} events",
            whole.len().div_ceil(page_size),
            whole.len()
        );
    }
}

/// A `NextToken` this service did not issue for this history is refused, rather than
/// honoured as a cursor into it.
///
/// The rows differ along why the token is not one of this projection's: it is empty, it is
/// not hex at all, it is a token of ANOTHER paginated read of this service, and it carries
/// this read's own marker with a cursor that is not a position.
#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    // The tokens are forged with `hex::encode`, the one hex encoder in this tree (the same
    // call `state::encode_cursor` and this crate's other token suites make).
    let cases = vec![
        ("empty", String::new()),
        ("not hex at all", "not-a-token".to_string()),
        (
            "a token of another paginated read",
            ::hex::encode("services/1/0"),
        ),
        (
            "this read's marker with no position",
            ::hex::encode("entity-events/1/first"),
        ),
    ];

    let router = seeded_router().await;
    let entity = service_entity("checkout", "prod");
    let now = now_seconds();

    for (label, token) in cases {
        let (status, answer, errortype) = list_events(
            &router,
            &entity,
            now - 3_600.0,
            now + 3_600.0,
            &format!("?NextToken={token}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert!(
            errortype.starts_with("ValidationException"),
            "case {label:?}: expected a ValidationException errortype, got {errortype:?}"
        );
    }
}

/// A request this read will not serve is refused with the modelled `ValidationException`
/// (HTTP 400) and nothing is reported: a range that does not run forwards, and an `Entity`
/// that identifies no entity — whose refusal names `entity`, the member THIS operation
/// carries.
///
/// The rows differ along the reason for the refusal (an inverted range, a range spanning no
/// time, an absent entity type, an unusable one) and, for the identity rows, along the size
/// and shape of the map that fails to identify anything.
#[tokio::test]
async fn a_request_this_read_will_not_serve_is_refused() {
    /// One refused request and the text its message must carry.
    struct Case {
        label: &'static str,
        entity: Value,
        start: f64,
        end: f64,
        requirement: &'static str,
    }

    let now = now_seconds();
    let cases = vec![
        Case {
            label: "an inverted range",
            entity: service_entity("checkout", "prod"),
            start: now,
            end: now - 3_600.0,
            requirement: "start time must be before end time",
        },
        Case {
            label: "a range spanning no time",
            entity: service_entity("checkout", "prod"),
            start: now,
            end: now,
            requirement: "start time must be before end time",
        },
        Case {
            label: "an entity naming no type",
            entity: json!({ "Name": "checkout", "Environment": "prod" }),
            start: now - 3_600.0,
            end: now + 3_600.0,
            requirement: "Value at 'entity' failed to satisfy constraint: Member must contain \
                          the entity type attribute 'Type'",
        },
        Case {
            label: "an entity whose type is not an entity type",
            entity: json!({ "Type": "service", "Name": "checkout" }),
            start: now - 3_600.0,
            end: now + 3_600.0,
            requirement: "Value at 'entity.Type' failed to satisfy constraint: Member must \
                          satisfy enum value set: [AWS::Resource, AWS::Service, RemoteService, \
                          Resource, Service]",
        },
        Case {
            label: "an entity whose only attribute is an unusable type",
            entity: json!({ "Type": "AWS::Logs::LogGroup" }),
            start: now - 3_600.0,
            end: now + 3_600.0,
            requirement: "Value at 'entity.Type' failed to satisfy constraint: Member must \
                          satisfy enum value set: [AWS::Resource, AWS::Service, RemoteService, \
                          Resource, Service]",
        },
    ];

    let router = seeded_router().await;
    for case in cases {
        let label = case.label;
        let (status, answer, errortype) =
            list_events(&router, &case.entity, case.start, case.end, "").await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert!(
            errortype.starts_with("ValidationException"),
            "case {label:?}: expected a ValidationException errortype, got {errortype:?}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(case.requirement),
            "case {label:?}: expected a message carrying {:?}, got {message:?}",
            case.requirement
        );
        assert!(
            answer.get("ChangeEvents").is_none(),
            "case {label:?}: a refusal reports no event, got {answer}"
        );
    }
}
