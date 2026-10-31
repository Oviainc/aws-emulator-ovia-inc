// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the boundary refused: the AWS constraint-violation
//! envelope, over every member the request broke a rule for.
//!
//! Every call goes over the wire, through the same [`MultiServiceRouter`] the production
//! emulator mounts, and what is asserted is what a client sees: the status, the error code
//! and the `message` of the 400 body.
//!
//! The rule under test, in one sentence: a refused request is answered
//! `N validation error(s) detected: ` over ONE clause per violated constraint, joined with
//! `; ` and ordered by the member each clause names, where a clause reads
//! `Value at '<member path>' failed to satisfy constraint: <requirement>` — it states no
//! value, neither the bytes a present member submitted nor the `null` an absent one would
//! be stated with — and a `@pattern` requirement is quoted without the `^`/`$` the model
//! writes around it.
//!
//! The tests are tables whose rows differ along what that rule generalises over: the
//! operation, the constraint class (absent member, pattern, each length bound), the number
//! of members one request breaks, and the form absence takes (an omitted key, an explicit
//! JSON `null`, no body at all).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state::{
    ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState as PersonalizeEventsState,
    MetadataKind,
};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for; the router resolves the service from the
/// credential scope.
const REGION: &str = "us-east-1";

/// The sigv4 signing name from the model (`smithy_query(query="service_info")`: sigv4Name
/// `personalize`).
const SIGNING_NAME: &str = "personalize";

/// The error code AWS answers a constraint refusal with on this protocol, and the one the
/// recorded refusals of this service carry.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `@pattern` the model declares on `datasetArn` (target `Arn`), as a refusal quotes
/// it: the model writes `^arn:([a-z\d-]+):personalize:.*:.*:.+$` and AWS publishes the
/// expression without the whole-value anchors.
const UNANCHORED_ARN_PATTERN: &str = r"arn:([a-z\d-]+):personalize:.*:.*:.+";

/// A dataset ARN matching that pattern, one per metadata route: the resource segment of a
/// Personalize dataset ARN is `dataset/<datasetGroup>/<TYPE>`.
const ITEMS_DATASET: &str = "arn:aws:personalize:us-east-1:123456789012:dataset/demo/ITEMS";
const USERS_DATASET: &str = "arn:aws:personalize:us-east-1:123456789012:dataset/demo/USERS";
const ACTIONS_DATASET: &str = "arn:aws:personalize:us-east-1:123456789012:dataset/demo/ACTIONS";

/// The tracking id the event-stream rows address.
const TRACKING_ID: &str = "tracker-1";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The `__type` of the error body, or the empty string for a body carrying none.
    code: String,
    /// The `message` of the error body, or the empty string for a body carrying none.
    message: String,
}

/// An emulator with just this service mounted on a fresh engine, driven in-process through
/// the production router.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST `body` to `uri`, exactly as the bytes given.
    async fn post_raw(&self, uri: &str, body: Vec<u8>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", format!("personalize-events.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(Bytes::from(body))
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).to_string();
        let parsed: Option<Value> = serde_json::from_str(&body).ok();
        let field = |name: &str| {
            parsed
                .as_ref()
                .and_then(|value| value.get(name))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Reply {
            status,
            code: field("__type"),
            message: field("message"),
        }
    }

    async fn post(&self, uri: &str, body: &Value) -> Reply {
        self.post_raw(uri, serde_json::to_vec(body).expect("serialize request body"))
            .await
    }

    /// Register an event tracker, as `CreateEventTracker` in the Personalize control plane
    /// would.
    fn seed_tracker(&self, tracking_id: &str) {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_event_tracker(tracking_id, None)
            .unwrap_or_else(|| panic!("the store accepts the tracker {tracking_id}"));
    }

    /// Register a dataset, as `CreateDataset` would.
    fn seed_dataset(&self, dataset_arn: &str) {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_dataset(dataset_arn)
            .unwrap_or_else(|| panic!("the store accepts the dataset {dataset_arn}"));
    }
}

/// The envelope `clauses` are stated in: the count, then the clauses joined with `; `.
fn envelope(clauses: &[&str]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The clause an ABSENT member is stated with.
fn absent_clause(member: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: Member must not be null")
}

/// The clause a value outside its member's `@pattern` is stated with.
fn pattern_clause(member: &str) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {UNANCHORED_ARN_PATTERN}"
    )
}

/// The clause a value or collection under its `@length` minimum is stated with.
fn min_length_clause(member: &str, min: usize) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: Member must have length greater than \
         or equal to {min}"
    )
}

/// The clause a value or collection over its `@length` maximum is stated with.
fn max_length_clause(member: &str, max: usize) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: Member must have length less than or \
         equal to {max}"
    )
}

/// Every route of this service, with the required members of its input and a body that
/// satisfies them. The member lists are the model's
/// (`smithy_query(query="operation",operation="PutActionInteractions,PutActions,PutEvents,PutItems,PutUsers")`;
/// the crate root's generated `OPERATIONS.md` lists the same sets as `required`).
struct Route {
    path: &'static str,
    required: &'static [&'static str],
}

const ROUTES: &[Route] = &[
    Route {
        path: "/action-interactions",
        required: &["actionInteractions", "trackingId"],
    },
    Route {
        path: "/actions",
        required: &["actions", "datasetArn"],
    },
    Route {
        path: "/events",
        required: &["eventList", "sessionId", "trackingId"],
    },
    Route {
        path: "/items",
        required: &["datasetArn", "items"],
    },
    Route {
        path: "/users",
        required: &["datasetArn", "users"],
    },
];

/// A request body for `route` that satisfies every constraint the model declares, with the
/// nested records carrying only the members their own shapes require.
fn valid_body(route: &str) -> Value {
    match route {
        "/action-interactions" => json!({
            "trackingId": TRACKING_ID,
            "actionInteractions": [{
                "actionId": "action-1",
                "sessionId": "session-1",
                "timestamp": 1_764_000_000.0,
                "eventType": "Taken",
            }],
        }),
        "/actions" => json!({
            "datasetArn": ACTIONS_DATASET,
            "actions": [{ "actionId": "action-1" }],
        }),
        "/events" => json!({
            "trackingId": TRACKING_ID,
            "sessionId": "session-1",
            "eventList": [{ "eventType": "click", "sentAt": 1_764_000_000.0 }],
        }),
        "/items" => json!({
            "datasetArn": ITEMS_DATASET,
            "items": [{ "itemId": "item-1" }],
        }),
        "/users" => json!({
            "datasetArn": USERS_DATASET,
            "users": [{ "userId": "user-1" }],
        }),
        other => panic!("no valid body for the route {other:?}"),
    }
}

/// `valid_body(route)` with `member` removed.
fn body_without(route: &str, member: &str) -> Value {
    let mut body = valid_body(route);
    body.as_object_mut()
        .expect("a request body is a JSON object")
        .remove(member)
        .unwrap_or_else(|| panic!("{route} carries {member}"));
    body
}

/// `valid_body(route)` with `member` replaced by `value`.
fn body_with(route: &str, member: &str, value: Value) -> Value {
    let mut body = valid_body(route);
    body.as_object_mut()
        .expect("a request body is a JSON object")
        .insert(member.to_string(), value);
    body
}

fn assert_refused(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} ({:?} {:?})",
        reply.status, reply.code, reply.message
    );
    assert!(
        reply.code.ends_with(VALIDATION_EXCEPTION),
        "case {case:?}: expected the error code {VALIDATION_EXCEPTION}, got {:?}",
        reply.code
    );
    assert_eq!(
        reply.message, expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        reply.message
    );
}

/// RULE: every member a request left out is stated, in one envelope whose count is the
/// number of clauses — not only the first member the deserializer could not fill.
#[tokio::test]
async fn every_member_a_request_left_out_is_stated_in_one_counted_envelope() {
    let emulator = Emulator::new();

    // (case, route, body bytes, the clauses expected, in order)
    let mut rows: Vec<(String, &str, Vec<u8>, Vec<String>)> = Vec::new();
    for route in ROUTES {
        let clauses: Vec<String> = route.required.iter().map(|m| absent_clause(m)).collect();
        // Every required member omitted: an empty object, the same object with each
        // member explicitly null, and no body at all — three forms of "carries no value".
        rows.push((
            format!("{} with an empty object", route.path),
            route.path,
            b"{}".to_vec(),
            clauses.clone(),
        ));
        let nulls: serde_json::Map<String, Value> = route
            .required
            .iter()
            .map(|member| ((*member).to_string(), Value::Null))
            .collect();
        rows.push((
            format!("{} with every required member null", route.path),
            route.path,
            serde_json::to_vec(&Value::Object(nulls)).expect("serialize"),
            clauses.clone(),
        ));
        rows.push((
            format!("{} with no body at all", route.path),
            route.path,
            Vec::new(),
            clauses,
        ));
        // Each required member omitted on its own: one clause, naming that member. This
        // is also what pins the required-member set to the model — a member the model does
        // NOT require would not be refused here at all.
        for member in route.required {
            rows.push((
                format!("{} without {member}", route.path),
                route.path,
                serde_json::to_vec(&body_without(route.path, member)).expect("serialize"),
                vec![absent_clause(member)],
            ));
        }
    }

    for (case, path, body, clauses) in rows {
        let borrowed: Vec<&str> = clauses.iter().map(String::as_str).collect();
        let reply = emulator.post_raw(path, body).await;
        assert_refused(&case, &reply, &envelope(&borrowed));
    }
}

/// RULE: a request carrying every required member is not refused for absence — it reaches
/// the handler, which serves it.
///
/// The non-vacuity guard for the test above: a gate that refused everything would pass it.
#[tokio::test]
async fn a_request_carrying_every_required_member_reaches_the_handler() {
    let emulator = Emulator::new();
    emulator.seed_tracker(TRACKING_ID);
    for dataset in [ITEMS_DATASET, USERS_DATASET, ACTIONS_DATASET] {
        emulator.seed_dataset(dataset);
    }

    for route in ROUTES {
        let reply = emulator.post(route.path, &valid_body(route.path)).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected the handler to serve a request that satisfies the model, got \
             {} ({:?} {:?})",
            route.path, reply.status, reply.code, reply.message
        );
        assert!(
            reply.message.is_empty(),
            "case {:?}: every operation of this service models an empty response body, got {:?}",
            route.path,
            reply.message
        );
    }
}

/// RULE: a `@pattern` requirement is quoted as the expression a value has to satisfy —
/// without the whole-value anchors the model writes around it — and the rejected value is
/// not echoed back.
#[tokio::test]
async fn a_pattern_requirement_is_quoted_unanchored_and_without_the_value() {
    let emulator = Emulator::new();

    // Values outside the pattern, differing along WHY they are outside it: unrelated text,
    // the empty string (which the member's `@length 0..256` admits), another service's
    // ARN, an ARN whose partition is not lower-case, and an ARN with too few segments.
    let outside: [&str; 5] = [
        "not-a-valid-personalize-arn",
        "",
        "arn:aws:s3:::example-bucket",
        "ARN:AWS:personalize:us-east-1:123456789012:dataset/demo/ITEMS",
        "arn:aws:personalize",
    ];

    for route in ROUTES {
        if !route.required.contains(&"datasetArn") {
            continue;
        }
        for value in outside {
            let case = format!("{} with datasetArn {value:?}", route.path);
            let body = body_with(route.path, "datasetArn", json!(value));
            let reply = emulator.post(route.path, &body).await;
            assert_refused(&case, &reply, &envelope(&[&pattern_clause("datasetArn")]));
            assert!(
                value.is_empty() || !reply.message.contains(value),
                "case {case:?}: the refusal must not echo the submitted value, got {:?}",
                reply.message
            );
            assert!(
                !reply.message.contains('^') && !reply.message.contains('$'),
                "case {case:?}: the pattern must be quoted without its whole-value anchors, got \
                 {:?}",
                reply.message
            );
        }
    }
}

/// RULE: a clause for a value or a collection outside a declared `@length` bound states
/// the bound and no value either — on each published bound of the member, and one step
/// past it.
#[tokio::test]
async fn a_length_bound_is_stated_without_the_value() {
    let emulator = Emulator::new();
    emulator.seed_tracker(TRACKING_ID);
    for dataset in [ITEMS_DATASET, USERS_DATASET, ACTIONS_DATASET] {
        emulator.seed_dataset(dataset);
    }

    // `datasetArn` is `@length 0..256`; an ARN one character past the maximum that still
    // satisfies the pattern breaks the length bound alone.
    let over_long_arn = format!(
        "{ITEMS_DATASET}{}",
        "x".repeat(257 - ITEMS_DATASET.len())
    );
    // `trackingId` is `@length 1..256`.
    let over_long_tracking_id = "t".repeat(257);
    // (case, route, body, the clauses expected)
    let rows: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "a datasetArn one character past its maximum",
            "/items",
            body_with("/items", "datasetArn", json!(over_long_arn)),
            vec![max_length_clause("datasetArn", 256)],
        ),
        (
            "an empty items list, under the collection minimum",
            "/items",
            body_with("/items", "items", json!([])),
            vec![min_length_clause("items", 1)],
        ),
        (
            "eleven items, one past the collection maximum",
            "/items",
            body_with(
                "/items",
                "items",
                Value::Array(
                    (0..11)
                        .map(|index| json!({ "itemId": format!("item-{index}") }))
                        .collect(),
                ),
            ),
            vec![max_length_clause("items", 10)],
        ),
        (
            "an empty trackingId, under its minimum",
            "/events",
            body_with("/events", "trackingId", json!("")),
            vec![min_length_clause("trackingId", 1)],
        ),
        (
            "a trackingId one character past its maximum",
            "/action-interactions",
            body_with(
                "/action-interactions",
                "trackingId",
                json!(over_long_tracking_id),
            ),
            vec![max_length_clause("trackingId", 256)],
        ),
        (
            "an empty actionInteractions list",
            "/action-interactions",
            body_with("/action-interactions", "actionInteractions", json!([])),
            vec![min_length_clause("actionInteractions", 1)],
        ),
    ];

    for (case, path, body, clauses) in rows {
        let borrowed: Vec<&str> = clauses.iter().map(String::as_str).collect();
        let reply = emulator.post(path, &body).await;
        assert_refused(case, &reply, &envelope(&borrowed));
        assert!(
            !reply.message.contains(&over_long_tracking_id[..10].repeat(2)),
            "case {case:?}: the refusal must not echo the submitted value, got {:?}",
            reply.message
        );
    }
}

/// RULE: a request that breaks several rules at once states one clause per violated
/// constraint, counted and ordered by the member each names — whether the members were
/// absent, out of bounds, or a mix of the two.
#[tokio::test]
async fn several_broken_rules_are_stated_as_several_counted_clauses() {
    let emulator = Emulator::new();

    // (case, route, body, the clauses expected, in the order expected)
    let rows: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "an empty trackingId and an empty actionInteractions list",
            "/action-interactions",
            json!({ "trackingId": "", "actionInteractions": [] }),
            vec![
                min_length_clause("actionInteractions", 1),
                min_length_clause("trackingId", 1),
            ],
        ),
        (
            "a datasetArn outside its pattern and an empty items list",
            "/items",
            json!({ "datasetArn": "nope", "items": [] }),
            vec![pattern_clause("datasetArn"), min_length_clause("items", 1)],
        ),
        (
            "an absent trackingId and an absent sessionId",
            "/events",
            json!({ "eventList": [{ "eventType": "click", "sentAt": 1_764_000_000.0 }] }),
            vec![absent_clause("sessionId"), absent_clause("trackingId")],
        ),
        (
            "an absent datasetArn and an absent users list",
            "/users",
            json!({}),
            vec![absent_clause("datasetArn"), absent_clause("users")],
        ),
    ];

    for (case, path, body, clauses) in rows {
        let borrowed: Vec<&str> = clauses.iter().map(String::as_str).collect();
        let reply = emulator.post(path, &body).await;
        assert_refused(case, &reply, &envelope(&borrowed));
    }
}

/// RULE: a refused request persists nothing — the refusal happens before the handler, so
/// the store is untouched whichever rule was broken.
#[tokio::test]
async fn a_refused_request_persists_nothing() {
    let emulator = Emulator::new();
    emulator.seed_tracker(TRACKING_ID);
    emulator.seed_dataset(ITEMS_DATASET);

    for (case, path, body) in [
        ("both members absent", "/action-interactions", json!({})),
        (
            "an absent trackingId",
            "/action-interactions",
            body_without("/action-interactions", "trackingId"),
        ),
        (
            "a datasetArn outside its pattern",
            "/items",
            body_with("/items", "datasetArn", json!("nope")),
        ),
    ] {
        let reply = emulator.post(path, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} ({:?})",
            reply.status, reply.message
        );
    }

    let state = emulator.engine.state::<PersonalizeEventsState>();
    let state = state.lock();
    assert!(
        state
            .event_trackers
            .get(TRACKING_ID)
            .is_some_and(|tracker| tracker.action_interactions.is_empty()),
        "a refused request must persist no action interaction"
    );
    assert!(
        state
            .datasets
            .get(ITEMS_DATASET)
            .is_some_and(|dataset| dataset.records(MetadataKind::Items).is_empty()),
        "a refused request must persist no item record"
    );
}
