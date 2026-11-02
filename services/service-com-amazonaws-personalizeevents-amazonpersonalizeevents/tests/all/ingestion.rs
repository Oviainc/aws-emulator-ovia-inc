// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Amazon Personalize Events ingestion.
//!
//! Every call goes over the wire: a request is built and dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and the status, the
//! `x-amzn-errortype` header and the (always empty) response body are what a client
//! sees. This service models EVERY operation's output as `Unit`, so there is no reader
//! to round-trip through: the only other thing a call produces is what it persists, and
//! these tests read that back through the shared engine's store — the same store the
//! handlers write, so a handler that answered 200 without persisting cannot pass.
//!
//! The tests are tables. Each row is one input along the axis the rule under test
//! generalises over (JSON form, reserved key name and casing, batch shape and ordering,
//! record count, metadata kind, target registration), and the `tracker-1` /
//! `item-1` case from the bootstrap issue is only one row.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state::{
    ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState as PersonalizeEventsState, Dataset,
    DatasetType, MetadataKind, Record, StoredActionInteraction, StoredEvent,
    ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE, INVALID_DATASET_ARN_MESSAGE,
    MAX_ACTION_INTERACTIONS_PER_TRACKER, MAX_EVENTS_PER_TRACKER, MAX_EVENT_TRACKERS,
    MAX_RECORDS_PER_DATASET_COLLECTION, RESERVED_PROPERTY_KEYS,
};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for. Nothing in this service is regional (it
/// mints no ARN), but the router resolves the service from the credential scope.
const REGION: &str = "us-east-1";

/// The sigv4 signing name from the model (`smithy_query(query="service_info")`:
/// sigv4Name `personalize`, endpoint prefix `personalize-events`).
const SIGNING_NAME: &str = "personalize";

/// A dataset ARN matching the `datasetArn` pattern the model declares
/// (`^arn:([a-z\d-]+):personalize:.*:.*:.+$`), one per dataset type: the resource
/// segment of a Personalize dataset ARN is `dataset/<datasetGroup>/<TYPE>`, so a test
/// addresses each metadata kind through the ARN of a dataset of that kind.
const ITEMS_DATASET: &str = "arn:aws:personalize:us-east-1:123456789012:dataset/demo/ITEMS";
const USERS_DATASET: &str = "arn:aws:personalize:us-east-1:123456789012:dataset/demo/USERS";
const ACTIONS_DATASET: &str = "arn:aws:personalize:us-east-1:123456789012:dataset/demo/ACTIONS";

/// One wire answer, reduced to what a client can branch on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    /// The raw body. Every successful response of this service is empty.
    body: String,
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router. The engine is kept so a test can seed the registry
/// the control plane would fill (`CreateEventTracker` / `CreateDataset` live in the
/// `personalize` service, not this one) and read back what ingestion persisted.
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

    async fn post(&self, uri: &str, body: &Value) -> Reply {
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
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize request body"),
            ))
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = String::from_utf8_lossy(&response.into_body()).to_string();
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// Register an event tracker, as `CreateEventTracker` in the Personalize control
    /// plane would.
    fn seed_tracker(&self, tracking_id: &str) {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_event_tracker(tracking_id, None)
            .unwrap_or_else(|| panic!("the store accepts the tracker {tracking_id}"));
    }

    /// Register a dataset, as `CreateDataset` in the Personalize control plane would.
    fn seed_dataset(&self, dataset_arn: &str) {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_dataset(dataset_arn)
            .unwrap_or_else(|| panic!("the store accepts the dataset {dataset_arn}"));
    }

    /// Mark a registered dataset busy, so the modelled `ResourceInUseException` is
    /// reachable the way it is on the real service (an import job holding it).
    fn mark_dataset_in_use(&self, dataset_arn: &str) {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_dataset(dataset_arn)
            .expect("the store accepts the dataset")
            .in_use = true;
    }

    /// Mark a registered tracker busy.
    fn mark_tracker_in_use(&self, tracking_id: &str) {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_event_tracker(tracking_id, None)
            .expect("the store accepts the tracker")
            .in_use = true;
    }

    fn stored_events(&self, tracking_id: &str) -> Vec<StoredEvent> {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .event_trackers
            .get(tracking_id)
            .map(|tracker| tracker.events.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn stored_interactions(&self, tracking_id: &str) -> Vec<StoredActionInteraction> {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .event_trackers
            .get(tracking_id)
            .map(|tracker| tracker.action_interactions.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many ids the tracker's item-interaction stream still de-duplicates on.
    fn remembered_event_ids(&self, tracking_id: &str) -> usize {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .event_trackers
            .get(tracking_id)
            .map_or(0, |tracker| tracker.event_ids.len())
    }

    /// How many ids the tracker's action-interaction stream still de-duplicates on.
    fn remembered_interaction_event_ids(&self, tracking_id: &str) -> usize {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .event_trackers
            .get(tracking_id)
            .map_or(0, |tracker| tracker.action_interaction_event_ids.len())
    }

    fn stored_records(&self, dataset_arn: &str, kind: MetadataKind) -> BTreeMap<String, Record> {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .datasets
            .get(dataset_arn)
            .map(|dataset: &Dataset| dataset.records(kind).clone())
            .unwrap_or_default()
    }

    /// The type the registry derived for a registered dataset, or `None` when it derived
    /// none (or the dataset is not registered at all).
    fn dataset_type(&self, dataset_arn: &str) -> Option<DatasetType> {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .datasets
            .get(dataset_arn)
            .and_then(|dataset: &Dataset| dataset.dataset_type)
    }
}

/// The event ids of a tracker's item-interaction stream, in ingestion order.
fn event_ids(events: &[StoredEvent]) -> Vec<String> {
    events.iter().map(|event| event.event_id.clone()).collect()
}

/// One `Event` with only the two members the model makes required, plus whatever extra
/// members the caller wants merged in.
fn event(event_type: &str, sent_at: f64, extra: Value) -> Value {
    let mut value = json!({ "eventType": event_type, "sentAt": sent_at });
    if let (Some(object), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        for (key, member) in extra {
            object.insert(key.clone(), member.clone());
        }
    }
    value
}

fn put_events_body(tracking_id: &str, session_id: &str, events: Vec<Value>) -> Value {
    json!({
        "trackingId": tracking_id,
        "sessionId": session_id,
        "userId": "user-1",
        "eventList": events,
    })
}

fn put_interactions_body(tracking_id: &str, interactions: Vec<Value>) -> Value {
    json!({ "trackingId": tracking_id, "actionInteractions": interactions })
}

/// One `ActionInteraction` with the four members the model makes required, plus extras.
fn interaction(action_id: &str, timestamp: f64, extra: Value) -> Value {
    let mut value = json!({
        "actionId": action_id,
        "sessionId": "session-1",
        "timestamp": timestamp,
        "eventType": "Taken",
    });
    if let (Some(object), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        for (key, member) in extra {
            object.insert(key.clone(), member.clone());
        }
    }
    value
}

fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} / {:?} {:?}",
        reply.status, reply.error_type, reply.body
    );
    assert!(
        reply.body.is_empty(),
        "case {case:?}: every operation of this service models an empty response body, \
         got {:?}",
        reply.body
    );
}

fn assert_refused(case: &str, reply: &Reply, status: u16, error_type: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} / {:?} {:?}",
        reply.status, reply.error_type, reply.body
    );
    assert_eq!(
        reply.error_type, error_type,
        "case {case:?}: expected {error_type}, got {:?} ({})",
        reply.error_type, reply.body
    );
}

/// Every route that takes a `properties` member, and whether the reserved-key rule
/// applies to it: it is documented on `Event$properties` and
/// `ActionInteraction$properties` only, so item/user/action metadata does not carry it.
const PROPERTY_ROUTES: [(&str, bool); 5] = [
    ("/events", true),
    ("/action-interactions", true),
    ("/items", false),
    ("/users", false),
    ("/actions", false),
];

/// How a `properties` string is classified, independently of the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A JSON object with no reserved key: always accepted.
    Object,
    /// A JSON object using a reserved key: refused on the event streams only.
    ReservedKey,
    /// Not a JSON object (malformed, or a non-object JSON value): always refused.
    NotAnObject,
}

/// A `properties` payload for the route `route`, in a request that is otherwise valid.
fn body_with_properties(route: &str, properties: &str) -> Value {
    match route {
        "/events" => put_events_body(
            "tracker-1",
            "session-1",
            vec![event("click", 1_764_000_000.0, json!({ "properties": properties }))],
        ),
        "/action-interactions" => put_interactions_body(
            "tracker-1",
            vec![interaction(
                "action-1",
                1_764_000_000.0,
                json!({ "properties": properties }),
            )],
        ),
        "/items" => json!({
            "datasetArn": ITEMS_DATASET,
            "items": [{ "itemId": "item-1", "properties": properties }],
        }),
        "/users" => json!({
            "datasetArn": USERS_DATASET,
            "users": [{ "userId": "user-1", "properties": properties }],
        }),
        "/actions" => json!({
            "datasetArn": ACTIONS_DATASET,
            "actions": [{ "actionId": "action-1", "properties": properties }],
        }),
        other => panic!("no body builder for {other}"),
    }
}

#[tokio::test]
async fn properties_must_be_a_json_object_and_the_event_streams_reserve_six_keys() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");
    emulator.seed_dataset(ITEMS_DATASET);
    emulator.seed_dataset(USERS_DATASET);
    emulator.seed_dataset(ACTIONS_DATASET);

    // Rows differ along the axis the rule generalises over: JSON form and value type,
    // which reserved name a key uses, and the casing it uses it in. The reserved names
    // themselves come from `RESERVED_PROPERTY_KEYS`, which cites the model docs.
    let mut cases: Vec<(String, Shape)> = vec![
        (r#"{"numberOfRatings":"12"}"#.to_string(), Shape::Object),
        ("{}".to_string(), Shape::Object),
        (
            r#"{"nested":{"a":[1,2,3]},"count":12,"flag":true,"absent":null}"#.to_string(),
            Shape::Object,
        ),
        // A key that merely CONTAINS a reserved name is not a reserved key.
        (
            r#"{"sessionIdentifier":"s","impressionCount":3}"#.to_string(),
            Shape::Object,
        ),
        (r#"{"numberOfRatings":}"#.to_string(), Shape::NotAnObject),
        (r#"{"numberOfRatings":"12""#.to_string(), Shape::NotAnObject),
        ("not json at all".to_string(), Shape::NotAnObject),
        (r#"[{"numberOfRatings":"12"}]"#.to_string(), Shape::NotAnObject),
        (r#""12""#.to_string(), Shape::NotAnObject),
        ("12".to_string(), Shape::NotAnObject),
    ];
    // Every reserved name, in three casings each — the rule is case insensitive, and the
    // issue cited only `sessionId`.
    for reserved in RESERVED_PROPERTY_KEYS {
        for key in [
            reserved.to_string(),
            reserved.to_uppercase(),
            reserved.to_lowercase(),
        ] {
            cases.push((
                format!("{{{:?}:\"x\",\"other\":1}}", key),
                Shape::ReservedKey,
            ));
        }
    }

    for (route, reserved_applies) in PROPERTY_ROUTES {
        for (properties, shape) in &cases {
            let case = format!("{route} properties={properties}");
            let reply = emulator
                .post(route, &body_with_properties(route, properties))
                .await;
            let refused = match shape {
                Shape::NotAnObject => true,
                Shape::ReservedKey => reserved_applies,
                Shape::Object => false,
            };
            if refused {
                assert_refused(&case, &reply, 400, "InvalidInputException");
            } else {
                assert_accepted(&case, &reply);
            }
        }
    }

    // A refused batch ingests nothing, so only the accepted rows are in the stream: the
    // four `Shape::Object` rows, each a distinct (generated) event id.
    let events = emulator.stored_events("tracker-1");
    assert_eq!(
        events.len(),
        4,
        "expected only the four accepted `properties` rows to be ingested, got {:?}",
        event_ids(&events)
    );
    assert!(
        events
            .iter()
            .all(|event| event.properties.as_ref().is_some_and(Value::is_object)),
        "expected every stored event to hold its properties as a JSON object, got {:?}",
        events
            .iter()
            .map(|event| event.properties.clone())
            .collect::<Vec<_>>()
    );
}

/// A batch is all-or-nothing: the call has one status, so one unusable record in it
/// means none of the batch is stored.
#[tokio::test]
async fn a_batch_with_one_unusable_record_ingests_none_of_it() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");
    emulator.seed_dataset(ITEMS_DATASET);

    let events = emulator
        .post(
            "/events",
            &put_events_body(
                "tracker-1",
                "session-1",
                vec![
                    event("click", 1.0, json!({ "eventId": "good-1" })),
                    event(
                        "click",
                        2.0,
                        json!({ "eventId": "bad-1", "properties": r#"{"sessionId":"x"}"# }),
                    ),
                ],
            ),
        )
        .await;
    assert_refused("mixed event batch", &events, 400, "InvalidInputException");
    assert!(
        emulator.stored_events("tracker-1").is_empty(),
        "case \"mixed event batch\": expected no event to be ingested, got {:?}",
        event_ids(&emulator.stored_events("tracker-1"))
    );

    let items = emulator
        .post(
            "/items",
            &json!({
                "datasetArn": ITEMS_DATASET,
                "items": [
                    { "itemId": "item-good", "properties": r#"{"genre":"Horror|Action"}"# },
                    { "itemId": "item-bad", "properties": "{" },
                ],
            }),
        )
        .await;
    assert_refused("mixed item batch", &items, 400, "InvalidInputException");
    assert!(
        emulator
            .stored_records(ITEMS_DATASET, MetadataKind::Items)
            .is_empty(),
        "case \"mixed item batch\": expected no item to be ingested, got {:?}",
        emulator.stored_records(ITEMS_DATASET, MetadataKind::Items)
    );
}

#[tokio::test]
async fn an_unregistered_target_is_refused_by_every_operation() {
    let emulator = Emulator::new();

    // Every operation whose target this emulator must resolve, with a target it does not
    // know. The dataset rows vary the ARN along its pattern (partition, region, account,
    // resource path); the tracker row varies the id form.
    //
    // A `datasetArn` that resolves to no dataset is an INPUT fault, not a not-found, and
    // the refusal names the member rather than echoing the ARN.
    let unknown_datasets = [
        "arn:aws:personalize:us-east-1:123456789012:dataset/unknown/ITEMS",
        "arn:aws-cn:personalize:cn-north-1:210987654321:dataset/other/USERS",
        "arn:aws-us-gov:personalize:us-gov-west-1::dataset/no-account/ACTIONS",
    ];
    for dataset_arn in unknown_datasets {
        for (route, body) in [
            (
                "/items",
                json!({ "datasetArn": dataset_arn, "items": [{ "itemId": "item-1" }] }),
            ),
            (
                "/users",
                json!({ "datasetArn": dataset_arn, "users": [{ "userId": "user-1" }] }),
            ),
            (
                "/actions",
                json!({ "datasetArn": dataset_arn, "actions": [{ "actionId": "action-1" }] }),
            ),
        ] {
            let case = format!("{route} datasetArn={dataset_arn}");
            let reply = emulator.post(route, &body).await;
            assert_refused(&case, &reply, 400, "InvalidInputException");
            assert!(
                reply.body.contains(INVALID_DATASET_ARN_MESSAGE)
                    && !reply.body.contains(dataset_arn),
                "case {case:?}: expected the message {INVALID_DATASET_ARN_MESSAGE:?} and no \
                 echo of the submitted ARN, got {:?}",
                reply.body
            );
        }
    }

    // A `trackingId` this service can ingest no action-interaction stream through is an
    // INPUT fault too, stated without echoing the submitted id.
    for tracking_id in ["unknown-tracker", "1234567890", "a"] {
        let case = format!("/action-interactions trackingId={tracking_id}");
        let reply = emulator
            .post(
                "/action-interactions",
                &put_interactions_body(
                    tracking_id,
                    vec![interaction("action-1", 1_764_000_000.0, json!({}))],
                ),
            )
            .await;
        assert_refused(&case, &reply, 400, "InvalidInputException");
        assert!(
            reply.body.contains(ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE),
            "case {case:?}: expected the message \
             {ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE:?}, got {:?}",
            reply.body
        );
        assert!(
            emulator.stored_interactions(tracking_id).is_empty(),
            "case {case:?}: a refused call must persist nothing, got {:?}",
            emulator.stored_interactions(tracking_id)
        );
    }

    // The target is resolved first: a request that is wrong about BOTH the target and a
    // record answers for the target, the stronger and more specific fault.
    for (case, route, body, status, error_type) in [
        (
            "/items unknown dataset and malformed properties",
            "/items",
            json!({
                "datasetArn": "arn:aws:personalize:us-east-1:123456789012:dataset/unknown/ITEMS",
                "items": [{ "itemId": "item-1", "properties": "{" }],
            }),
            400,
            "InvalidInputException",
        ),
        (
            "/users unknown dataset and malformed properties",
            "/users",
            json!({
                "datasetArn": "arn:aws:personalize:us-east-1:123456789012:dataset/unknown/USERS",
                "users": [{ "userId": "user-1", "properties": "not json" }],
            }),
            400,
            "InvalidInputException",
        ),
        (
            "/action-interactions unknown tracker and a reserved properties key",
            "/action-interactions",
            json!({
                "trackingId": "unknown-tracker",
                "actionInteractions": [{
                    "actionId": "action-1",
                    "sessionId": "session-1",
                    "timestamp": 1_764_000_000,
                    "eventType": "Taken",
                    "properties": r#"{"sessionId":"x"}"#,
                }],
            }),
            400,
            "InvalidInputException",
        ),
    ] {
        let reply = emulator.post(route, &body).await;
        assert_refused(case, &reply, status, error_type);
    }

    // Registering the targets is the only difference, and each now ingests through the
    // operation that matches the dataset type its ARN names.
    for (dataset_arn, route, kind, body) in [
        (
            unknown_datasets[0],
            "/items",
            MetadataKind::Items,
            json!({ "datasetArn": unknown_datasets[0], "items": [{ "itemId": "item-1" }] }),
        ),
        (
            unknown_datasets[1],
            "/users",
            MetadataKind::Users,
            json!({ "datasetArn": unknown_datasets[1], "users": [{ "userId": "user-1" }] }),
        ),
        (
            unknown_datasets[2],
            "/actions",
            MetadataKind::Actions,
            json!({ "datasetArn": unknown_datasets[2], "actions": [{ "actionId": "action-1" }] }),
        ),
    ] {
        emulator.seed_dataset(dataset_arn);
        let case = format!("registered {route} datasetArn={dataset_arn}");
        let reply = emulator.post(route, &body).await;
        assert_accepted(&case, &reply);
        let stored = emulator.stored_records(dataset_arn, kind);
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: expected exactly the record it carried, got {:?}",
            stored.keys().collect::<Vec<_>>()
        );
    }

    for tracking_id in ["unknown-tracker", "1234567890", "a"] {
        emulator.seed_tracker(tracking_id);
        let case = format!("registered /action-interactions trackingId={tracking_id}");
        let reply = emulator
            .post(
                "/action-interactions",
                &put_interactions_body(
                    tracking_id,
                    vec![interaction("action-1", 1_764_000_000.0, json!({}))],
                ),
            )
            .await;
        assert_accepted(&case, &reply);
        assert_eq!(
            emulator.stored_interactions(tracking_id).len(),
            1,
            "case {case:?}: expected one persisted interaction, got {:?}",
            emulator.stored_interactions(tracking_id)
        );
    }
}

/// `PutEvents` serves only a tracker this emulator knows, like its four siblings; it
/// models no `ResourceNotFoundException`, so the refusal is its one modelled error.
#[tokio::test]
async fn put_events_refuses_a_tracking_id_it_does_not_know() {
    let emulator = Emulator::new();

    // Rows vary the id form: words, digits, the 1-char minimum, the 256-char maximum.
    let unknown = [
        "fresh-tracker".to_string(),
        "1234567890".to_string(),
        "a".to_string(),
        "t".repeat(256),
    ];
    for tracking_id in &unknown {
        let case = format!("/events trackingId={tracking_id}");
        let reply = emulator
            .post(
                "/events",
                &put_events_body(
                    tracking_id,
                    "session-1",
                    vec![event("click", 1_764_000_000.0, json!({ "eventId": "e-1" }))],
                ),
            )
            .await;
        assert_refused(&case, &reply, 400, "InvalidInputException");
        assert!(
            emulator.stored_events(tracking_id).is_empty(),
            "case {case:?}: a refused call must persist nothing, got {:?}",
            event_ids(&emulator.stored_events(tracking_id))
        );
    }

    // Registering the tracker is the only difference, and the identical call ingests.
    for tracking_id in &unknown {
        emulator.seed_tracker(tracking_id);
        let case = format!("registered /events trackingId={tracking_id}");
        let reply = emulator
            .post(
                "/events",
                &put_events_body(
                    tracking_id,
                    "session-1",
                    vec![event("click", 1_764_000_000.0, json!({ "eventId": "e-1" }))],
                ),
            )
            .await;
        assert_accepted(&case, &reply);
        assert_eq!(
            event_ids(&emulator.stored_events(tracking_id)),
            vec!["e-1".to_string()],
            "case {case:?}: expected the event in that tracker's own stream"
        );
    }

    // Registering a tracker that already ingested is idempotent: the stream stands.
    emulator.seed_tracker("fresh-tracker");
    assert_eq!(
        event_ids(&emulator.stored_events("fresh-tracker")),
        vec!["e-1".to_string()],
        "registering an existing tracker must not discard its stream"
    );

    // A tracker marked in use refuses `PutActionInteractions` with the modelled 409 —
    // and still ingests events, because `PutEvents` models no such error.
    emulator.mark_tracker_in_use("fresh-tracker");
    let reply = emulator
        .post(
            "/action-interactions",
            &put_interactions_body(
                "fresh-tracker",
                vec![interaction("action-1", 1_764_000_000.0, json!({}))],
            ),
        )
        .await;
    assert_refused(
        "/action-interactions on an in-use tracker",
        &reply,
        409,
        "ResourceInUseException",
    );
    assert_accepted(
        "/events on an in-use tracker",
        &emulator
            .post(
                "/events",
                &put_events_body(
                    "fresh-tracker",
                    "session-1",
                    vec![event("click", 2.0, json!({ "eventId": "e-2" }))],
                ),
            )
            .await,
    );
}

#[tokio::test]
async fn an_in_use_dataset_refuses_every_metadata_put() {
    let emulator = Emulator::new();
    for dataset_arn in [ITEMS_DATASET, USERS_DATASET, ACTIONS_DATASET] {
        emulator.mark_dataset_in_use(dataset_arn);
    }

    for (route, dataset_arn, kind, body) in [
        (
            "/items",
            ITEMS_DATASET,
            MetadataKind::Items,
            json!({ "datasetArn": ITEMS_DATASET, "items": [{ "itemId": "item-1" }] }),
        ),
        (
            "/users",
            USERS_DATASET,
            MetadataKind::Users,
            json!({ "datasetArn": USERS_DATASET, "users": [{ "userId": "user-1" }] }),
        ),
        (
            "/actions",
            ACTIONS_DATASET,
            MetadataKind::Actions,
            json!({ "datasetArn": ACTIONS_DATASET, "actions": [{ "actionId": "action-1" }] }),
        ),
    ] {
        let case = format!("{route} on an in-use dataset");
        let reply = emulator.post(route, &body).await;
        assert_refused(&case, &reply, 409, "ResourceInUseException");
        assert!(
            emulator.stored_records(dataset_arn, kind).is_empty(),
            "case {case:?}: a refused call must persist nothing, got {:?}",
            emulator.stored_records(dataset_arn, kind)
        );
    }
}

/// One batch of event ids as sent, and the stream the tracker must hold afterwards.
struct DedupCase {
    label: &'static str,
    /// `None` is an event whose caller omitted `eventId`.
    batch: Vec<Option<&'static str>>,
    /// The caller-supplied ids expected in the stream, in order.
    expected_supplied: Vec<&'static str>,
    /// How many events of the batch get a generated id.
    expected_generated: usize,
}

#[tokio::test]
async fn an_event_id_identifies_one_event_and_an_omitted_one_is_generated() {
    // Rows differ along batch size, how often an id repeats, where the repeat sits, and
    // how many ids are omitted. The rule: an id the tracker has already seen is ingested
    // once ("Any subsequent events after the first with the same event ID are not
    // used"), and an omitted id is generated.
    let cases = [
        DedupCase {
            label: "one id, sent once",
            batch: vec![Some("e-1")],
            expected_supplied: vec!["e-1"],
            expected_generated: 0,
        },
        DedupCase {
            label: "the same id twice in one batch",
            batch: vec![Some("e-1"), Some("e-1")],
            expected_supplied: vec!["e-1"],
            expected_generated: 0,
        },
        DedupCase {
            label: "a repeat between two distinct ids",
            batch: vec![Some("e-1"), Some("e-2"), Some("e-1"), Some("e-3")],
            expected_supplied: vec!["e-1", "e-2", "e-3"],
            expected_generated: 0,
        },
        DedupCase {
            label: "every id omitted",
            batch: vec![None, None, None],
            expected_supplied: vec![],
            expected_generated: 3,
        },
        DedupCase {
            label: "omitted ids around a repeated one",
            batch: vec![Some("e-1"), None, Some("e-1"), None],
            expected_supplied: vec!["e-1"],
            expected_generated: 2,
        },
        DedupCase {
            label: "a full batch of ten, half of them repeats",
            batch: vec![
                Some("a"),
                Some("b"),
                Some("c"),
                Some("d"),
                Some("e"),
                Some("a"),
                Some("b"),
                Some("c"),
                Some("d"),
                Some("e"),
            ],
            expected_supplied: vec!["a", "b", "c", "d", "e"],
            expected_generated: 0,
        },
    ];

    for case in &cases {
        let label = case.label;
        // A fresh emulator per row, so each row's stream is only its own batch.
        let emulator = Emulator::new();
        emulator.seed_tracker("tracker-1");

        let events: Vec<Value> = case
            .batch
            .iter()
            .enumerate()
            .map(|(index, event_id)| {
                let extra = match event_id {
                    Some(id) => json!({ "eventId": id }),
                    None => json!({}),
                };
                event("click", 1_764_000_000.0 + index as f64, extra)
            })
            .collect();
        let body = put_events_body("tracker-1", "session-1", events);

        let first = emulator.post("/events", &body).await;
        assert_accepted(label, &first);

        let expected_len = case.expected_supplied.len() + case.expected_generated;
        let stored = emulator.stored_events("tracker-1");
        assert_eq!(
            stored.len(),
            expected_len,
            "case {label:?}: expected {expected_len} stored events, got {:?}",
            event_ids(&stored)
        );
        let supplied: Vec<String> = stored
            .iter()
            .map(|event| event.event_id.clone())
            .filter(|id| case.expected_supplied.contains(&id.as_str()))
            .collect();
        assert_eq!(
            supplied,
            case.expected_supplied
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {label:?}: expected the caller's ids once each, in arrival order, got {:?}",
            event_ids(&stored)
        );
        let generated: Vec<String> = event_ids(&stored)
            .into_iter()
            .filter(|id| !case.expected_supplied.contains(&id.as_str()))
            .collect();
        assert_eq!(
            generated.len(),
            case.expected_generated,
            "case {label:?}: expected {} generated ids, got {generated:?}",
            case.expected_generated
        );
        let mut distinct = generated.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            generated.len(),
            "case {label:?}: a generated event id must be unique, got {generated:?}"
        );
        assert!(
            generated.iter().all(|id| !id.is_empty() && id.len() <= 256),
            "case {label:?}: a generated event id must satisfy length 1..256, got {generated:?}"
        );

        // Replaying the identical call is not a second ingestion of the ids it carried:
        // only its omitted ids become new events.
        let replay = emulator.post("/events", &body).await;
        assert_accepted(label, &replay);
        let after = emulator.stored_events("tracker-1");
        assert_eq!(
            after.len(),
            expected_len + case.expected_generated,
            "case {label:?}: a replay must re-ingest nothing it identified by id, got {:?}",
            event_ids(&after)
        );
    }
}

#[tokio::test]
async fn an_action_interaction_id_identifies_one_interaction_too() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");

    let body = put_interactions_body(
        "tracker-1",
        vec![
            interaction("action-1", 1.0, json!({ "eventId": "i-1" })),
            interaction("action-2", 2.0, json!({ "eventId": "i-1" })),
            interaction("action-3", 3.0, json!({})),
        ],
    );
    assert_accepted("interaction batch", &emulator.post("/action-interactions", &body).await);

    let stored = emulator.stored_interactions("tracker-1");
    assert_eq!(
        stored.len(),
        2,
        "expected the repeated interaction id to be ingested once, got {:?}",
        stored
            .iter()
            .map(|held| (held.event_id.clone(), held.action_id.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        stored[0].action_id, "action-1",
        "expected the FIRST interaction with an id to stand, got {:?}",
        stored[0]
    );
    assert!(
        !stored[1].event_id.is_empty(),
        "expected a generated id for the interaction that omitted one, got {:?}",
        stored[1]
    );

    let replay = emulator.post("/action-interactions", &body).await;
    assert_accepted("interaction replay", &replay);
    assert_eq!(
        emulator.stored_interactions("tracker-1").len(),
        3,
        "a replay re-ingests only what it did not identify by id, got {:?}",
        emulator.stored_interactions("tracker-1").len()
    );
}

/// The two streams feed two different datasets (Item interactions and Action
/// interactions), so each de-duplicates on its OWN ids: one id used in both names two
/// different events.
#[tokio::test]
async fn the_two_streams_of_a_tracker_de_duplicate_independently() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");

    for shared_id in ["shared-1", "shared-2"] {
        assert_accepted(
            shared_id,
            &emulator
                .post(
                    "/events",
                    &put_events_body(
                        "tracker-1",
                        "session-1",
                        vec![event("click", 1.0, json!({ "eventId": shared_id }))],
                    ),
                )
                .await,
        );
        assert_accepted(
            shared_id,
            &emulator
                .post(
                    "/action-interactions",
                    &put_interactions_body(
                        "tracker-1",
                        vec![interaction("action-1", 1.0, json!({ "eventId": shared_id }))],
                    ),
                )
                .await,
        );
    }

    assert_eq!(
        event_ids(&emulator.stored_events("tracker-1")),
        vec!["shared-1".to_string(), "shared-2".to_string()],
        "expected the item-interaction stream to hold both ids once"
    );
    assert_eq!(
        emulator
            .stored_interactions("tracker-1")
            .iter()
            .map(|held| held.event_id.clone())
            .collect::<Vec<_>>(),
        vec!["shared-1".to_string(), "shared-2".to_string()],
        "expected the action-interaction stream to hold the same ids once each: an id of \
         the other stream must not hide an interaction"
    );
}

/// One metadata kind: the route, the dataset of that kind it is addressed through, and
/// how a record of it is named in a request.
struct MetadataCase {
    route: &'static str,
    dataset: &'static str,
    kind: MetadataKind,
    collection: &'static str,
    id_member: &'static str,
}

const METADATA_CASES: [MetadataCase; 3] = [
    MetadataCase {
        route: "/items",
        dataset: ITEMS_DATASET,
        kind: MetadataKind::Items,
        collection: "items",
        id_member: "itemId",
    },
    MetadataCase {
        route: "/users",
        dataset: USERS_DATASET,
        kind: MetadataKind::Users,
        collection: "users",
        id_member: "userId",
    },
    MetadataCase {
        route: "/actions",
        dataset: ACTIONS_DATASET,
        kind: MetadataKind::Actions,
        collection: "actions",
        id_member: "actionId",
    },
];

/// One round of metadata puts: the records as SENT, and every record the dataset must
/// hold afterwards. Each entry is `(id, raw properties)`.
struct MetadataRound {
    label: &'static str,
    batch: &'static [(&'static str, Option<&'static str>)],
    expected: &'static [(&'static str, Option<&'static str>)],
}

#[tokio::test]
async fn a_repeated_metadata_id_replaces_its_record_rather_than_adding_one() {
    let emulator = Emulator::new();
    for case in &METADATA_CASES {
        emulator.seed_dataset(case.dataset);
    }

    // Rows differ along record count, which ids repeat, and the properties the repeat
    // carries (a new value, a different key set, or none at all).
    let rounds = [
        MetadataRound {
            label: "first put of two records",
            batch: &[
                ("record-1", Some(r#"{"genre":"Horror|Action"}"#)),
                ("record-2", None),
            ],
            expected: &[
                ("record-1", Some(r#"{"genre":"Horror|Action"}"#)),
                ("record-2", None),
            ],
        },
        MetadataRound {
            label: "re-put one id with new properties",
            batch: &[("record-1", Some(r#"{"genre":"Comedy"}"#))],
            expected: &[
                ("record-1", Some(r#"{"genre":"Comedy"}"#)),
                ("record-2", None),
            ],
        },
        MetadataRound {
            label: "re-put one id with a different key set",
            batch: &[("record-2", Some(r#"{"a":1,"b":{"c":2}}"#))],
            expected: &[
                ("record-1", Some(r#"{"genre":"Comedy"}"#)),
                ("record-2", Some(r#"{"a":1,"b":{"c":2}}"#)),
            ],
        },
        MetadataRound {
            label: "re-put one id with no properties, and add a third",
            batch: &[("record-1", None), ("record-3", Some("{}"))],
            expected: &[
                ("record-1", None),
                ("record-2", Some(r#"{"a":1,"b":{"c":2}}"#)),
                ("record-3", Some("{}")),
            ],
        },
    ];

    for case in &METADATA_CASES {
        for round in &rounds {
            let MetadataRound {
                label,
                batch,
                expected,
            } = round;
            let records: Vec<Value> = batch
                .iter()
                .map(|(id, properties)| {
                    let mut value = json!({ case.id_member: id });
                    if let Some(properties) = properties {
                        value
                            .as_object_mut()
                            .expect("record is an object")
                            .insert("properties".to_string(), json!(properties));
                    }
                    value
                })
                .collect();
            let body = json!({
                "datasetArn": case.dataset,
                case.collection: records,
            });

            let case_label = format!("{} {label}", case.route);
            assert_accepted(&case_label, &emulator.post(case.route, &body).await);

            let stored = emulator.stored_records(case.dataset, case.kind);
            assert_eq!(
                stored.keys().cloned().collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|(id, _)| (*id).to_string())
                    .collect::<Vec<_>>(),
                "case {case_label:?}: expected one record per id, got {:?}",
                stored.keys().collect::<Vec<_>>()
            );
            for (id, properties) in expected.iter() {
                let record = stored.get(*id).expect("record just asserted present");
                let expected_properties: Option<Value> = properties
                    .map(|raw| serde_json::from_str(raw).expect("expected properties are JSON"));
                assert_eq!(
                    record.properties, expected_properties,
                    "case {case_label:?}: expected {id} to hold {expected_properties:?}, got {:?}",
                    record.properties
                );
                assert_eq!(
                    record.id, *id,
                    "case {case_label:?}: a record must know its own id, got {:?}",
                    record.id
                );
            }
        }
    }
}

/// The whole of an event is persisted: the members it carried, and the session and user
/// of the call that carried it (those are per request, not per event).
#[tokio::test]
async fn an_ingested_event_keeps_every_member_and_the_calls_session_and_user() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");

    // `sentAt` rows span the epoch-seconds axis: zero, a whole second, and a fractional
    // one (restJson1 binds a timestamp in a JSON body as epoch seconds).
    for (label, sent_at) in [
        ("epoch", 0.0_f64),
        ("whole seconds", 1_764_000_000.0),
        ("fractional seconds", 1_764_000_000.25),
    ] {
        let body = json!({
            "trackingId": "tracker-1",
            "sessionId": format!("session-{label}"),
            "userId": "user-1",
            "eventList": [{
                "eventId": format!("e-{label}"),
                "eventType": "watch",
                "sentAt": sent_at,
                "itemId": "item-2",
                "eventValue": 2.5,
                "properties": r#"{"numberOfRatings":"12"}"#,
                "recommendationId": "rid-1",
                "impression": ["item-1", "item-2", "item-3"],
                "metricAttribution": { "eventAttributionSource": "thirdPartySite" },
            }],
        });
        assert_accepted(label, &emulator.post("/events", &body).await);

        let stored = emulator.stored_events("tracker-1");
        let event = stored
            .iter()
            .find(|event| event.event_id == format!("e-{label}"))
            .unwrap_or_else(|| panic!("case {label:?}: event not ingested, got {stored:?}"));
        assert_eq!(
            event.session_id,
            format!("session-{label}"),
            "case {label:?}: expected the call's sessionId, got {:?}",
            event.session_id
        );
        assert_eq!(
            event.user_id.as_deref(),
            Some("user-1"),
            "case {label:?}: expected the call's userId, got {:?}",
            event.user_id
        );
        assert_eq!(
            event.event_type, "watch",
            "case {label:?}: expected the event type as sent, got {:?}",
            event.event_type
        );
        let seconds = event.sent_at.as_secs_f64();
        assert!(
            (seconds - sent_at).abs() < 1e-6,
            "case {label:?}: expected sentAt {sent_at}, got {seconds}"
        );
        assert_eq!(
            event.item_id.as_deref(),
            Some("item-2"),
            "case {label:?}: expected the itemId as sent, got {:?}",
            event.item_id
        );
        assert_eq!(
            event.event_value,
            Some(2.5),
            "case {label:?}: expected the eventValue as sent, got {:?}",
            event.event_value
        );
        assert_eq!(
            event.properties,
            Some(json!({ "numberOfRatings": "12" })),
            "case {label:?}: expected the properties as a JSON object, got {:?}",
            event.properties
        );
        assert_eq!(
            event.recommendation_id.as_deref(),
            Some("rid-1"),
            "case {label:?}: expected the recommendationId as sent, got {:?}",
            event.recommendation_id
        );
        assert_eq!(
            event.impression,
            vec![
                "item-1".to_string(),
                "item-2".to_string(),
                "item-3".to_string()
            ],
            "case {label:?}: expected the impression list in order, got {:?}",
            event.impression
        );
        assert_eq!(
            event
                .metric_attribution
                .as_ref()
                .map(|attribution| attribution.event_attribution_source.clone()),
            Some("thirdPartySite".to_string()),
            "case {label:?}: expected the metric attribution source, got {:?}",
            event.metric_attribution
        );
    }
}

/// The bootstrap issue's own sequence, end to end: seed, put metadata, put events,
/// replay, put action interactions, and an unknown dataset.
#[tokio::test]
async fn the_reported_ingestion_sequence_works_end_to_end() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");
    // The issue's step 1 seeds one ARN; each metadata kind is addressed through the
    // dataset of its own type, which is how the real control plane mints them.
    emulator.seed_dataset(ITEMS_DATASET);
    emulator.seed_dataset(USERS_DATASET);
    emulator.seed_dataset(ACTIONS_DATASET);

    assert_accepted(
        "PutItems",
        &emulator
            .post(
                "/items",
                &json!({
                    "datasetArn": ITEMS_DATASET,
                    "items": [
                        { "itemId": "item-1", "properties": r#"{"genre":"Horror|Action"}"# },
                        { "itemId": "item-2" },
                    ],
                }),
            )
            .await,
    );
    assert_accepted(
        "PutUsers",
        &emulator
            .post(
                "/users",
                &json!({
                    "datasetArn": USERS_DATASET,
                    "users": [{ "userId": "user-1", "properties": r#"{"numberOfVideosWatched":"45"}"# }],
                }),
            )
            .await,
    );
    assert_accepted(
        "PutActions",
        &emulator
            .post(
                "/actions",
                &json!({
                    "datasetArn": ACTIONS_DATASET,
                    "actions": [{ "actionId": "action-1", "properties": r#"{"value":"100"}"# }],
                }),
            )
            .await,
    );

    let events = put_events_body(
        "tracker-1",
        "session-1",
        vec![
            event(
                "click",
                1_764_000_000.0,
                json!({ "eventId": "e-1", "itemId": "item-1", "eventValue": 1.0 }),
            ),
            event("watch", 1_764_000_001.0, json!({ "itemId": "item-2" })),
        ],
    );
    assert_accepted("PutEvents", &emulator.post("/events", &events).await);
    let after_first = emulator.stored_events("tracker-1");
    assert_eq!(
        after_first.len(),
        2,
        "expected both events in the stream, got {:?}",
        event_ids(&after_first)
    );

    assert_accepted("PutEvents replay", &emulator.post("/events", &events).await);
    let after_replay = emulator.stored_events("tracker-1");
    assert_eq!(
        after_replay
            .iter()
            .filter(|event| event.event_id == "e-1")
            .count(),
        1,
        "expected e-1 to be held once after the replay, got {:?}",
        event_ids(&after_replay)
    );

    assert_accepted(
        "PutActionInteractions",
        &emulator
            .post(
                "/action-interactions",
                &put_interactions_body(
                    "tracker-1",
                    vec![interaction(
                        "action-1",
                        1_764_000_002.0,
                        json!({ "userId": "user-1" }),
                    )],
                ),
            )
            .await,
    );
    assert_eq!(
        emulator.stored_interactions("tracker-1").len(),
        1,
        "expected one persisted action interaction, got {:?}",
        emulator.stored_interactions("tracker-1")
    );

    let unknown = emulator
        .post(
            "/items",
            &json!({
                "datasetArn": "arn:aws:personalize:us-east-1:123456789012:dataset/unknown/ITEMS",
                "items": [{ "itemId": "item-1" }],
            }),
        )
        .await;
    assert_refused("PutItems unknown dataset", &unknown, 400, "InvalidInputException");
}

// ---------------------------------------------------------------------------
// The store's capacity: this emulator ingests forever, so every growth site is
// bounded. These are the bounds `src/state.rs` documents.
// ---------------------------------------------------------------------------

/// The modelled constraints are refused before a handler runs, so an ingestion call is
/// never evaluated against a request the real service would reject outright.
#[tokio::test]
async fn the_modelled_constraints_on_a_request_are_refused_with_400() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");
    emulator.seed_dataset(ITEMS_DATASET);

    let over_long = "x".repeat(257);
    let eleven_items: Vec<Value> = (0..11)
        .map(|index| json!({ "itemId": format!("item-{index}") }))
        .collect();
    let eleven_events: Vec<Value> = (0..11)
        .map(|index| event("click", index as f64, json!({})))
        .collect();

    // Each row breaks ONE modelled constraint: list length 1..10, id length 1..256,
    // the `datasetArn` pattern, a `@required` member, or `properties` length 1..N.
    let cases: Vec<(String, &str, Value)> = vec![
        (
            "items over the modelled maximum of 10".to_string(),
            "/items",
            json!({ "datasetArn": ITEMS_DATASET, "items": eleven_items }),
        ),
        (
            "an empty items list".to_string(),
            "/items",
            json!({ "datasetArn": ITEMS_DATASET, "items": [] }),
        ),
        (
            "an empty itemId".to_string(),
            "/items",
            json!({ "datasetArn": ITEMS_DATASET, "items": [{ "itemId": "" }] }),
        ),
        (
            "an itemId over 256 characters".to_string(),
            "/items",
            json!({ "datasetArn": ITEMS_DATASET, "items": [{ "itemId": over_long }] }),
        ),
        (
            "a datasetArn that is not an ARN".to_string(),
            "/items",
            json!({ "datasetArn": "not-an-arn", "items": [{ "itemId": "item-1" }] }),
        ),
        (
            "an empty datasetArn".to_string(),
            "/items",
            json!({ "datasetArn": "", "items": [{ "itemId": "item-1" }] }),
        ),
        (
            "an empty item properties string".to_string(),
            "/items",
            json!({
                "datasetArn": ITEMS_DATASET,
                "items": [{ "itemId": "item-1", "properties": "" }],
            }),
        ),
        (
            "events over the modelled maximum of 10".to_string(),
            "/events",
            put_events_body("tracker-1", "session-1", eleven_events),
        ),
        (
            "an event without its required eventType".to_string(),
            "/events",
            json!({
                "trackingId": "tracker-1",
                "sessionId": "session-1",
                "eventList": [{ "sentAt": 1_764_000_000 }],
            }),
        ),
        (
            "an empty sessionId".to_string(),
            "/events",
            put_events_body("tracker-1", "", vec![event("click", 1.0, json!({}))]),
        ),
        (
            "a trackingId over 256 characters".to_string(),
            "/events",
            put_events_body(
                &"t".repeat(257),
                "session-1",
                vec![event("click", 1.0, json!({}))],
            ),
        ),
        (
            "an action interaction without its required actionId".to_string(),
            "/action-interactions",
            json!({
                "trackingId": "tracker-1",
                "actionInteractions": [{
                    "sessionId": "session-1",
                    "timestamp": 1_764_000_000,
                    "eventType": "Taken",
                }],
            }),
        ),
    ];

    for (case, route, body) in cases {
        let reply = emulator.post(route, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected the modelled constraint to be refused with HTTP 400, \
             got {} / {:?} {:?}",
            reply.status, reply.error_type, reply.body
        );
    }

    // Not one of them reached the store.
    assert!(
        emulator.stored_events("tracker-1").is_empty()
            && emulator.stored_interactions("tracker-1").is_empty()
            && emulator
                .stored_records(ITEMS_DATASET, MetadataKind::Items)
                .is_empty(),
        "a refused request must persist nothing, got {:?} / {:?} / {:?}",
        event_ids(&emulator.stored_events("tracker-1")),
        emulator.stored_interactions("tracker-1").len(),
        emulator.stored_records(ITEMS_DATASET, MetadataKind::Items)
    );
}

/// The modelled maximum batch size, shared by `eventList`, `actionInteractions`,
/// `items`, `users` and `actions` (all `len 1..10`).
const MAX_BATCH: usize = 10;

/// Ingest `count` events with ids `{prefix}-{n}` into `tracking_id`, in batches of the
/// modelled maximum, requiring every call to be accepted.
async fn ingest_events(emulator: &Emulator, tracking_id: &str, prefix: &str, count: usize) {
    for batch in 0..count / MAX_BATCH {
        let events: Vec<Value> = (0..MAX_BATCH)
            .map(|offset| {
                let index = batch * MAX_BATCH + offset;
                event(
                    "click",
                    index as f64,
                    json!({ "eventId": format!("{prefix}-{index}") }),
                )
            })
            .collect();
        let case = format!("batch {batch} of {prefix}");
        assert_accepted(
            &case,
            &emulator
                .post("/events", &put_events_body(tracking_id, "session-1", events))
                .await,
        );
    }
}

#[tokio::test]
async fn an_event_stream_retains_its_most_recent_records_and_forgets_what_it_dropped() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");

    ingest_events(&emulator, "tracker-1", "e", MAX_EVENTS_PER_TRACKER).await;
    let full = emulator.stored_events("tracker-1");
    assert_eq!(
        full.len(),
        MAX_EVENTS_PER_TRACKER,
        "case \"at the cap\": expected the stream to hold its cap, got {}",
        full.len()
    );
    assert_eq!(
        emulator.remembered_event_ids("tracker-1"),
        MAX_EVENTS_PER_TRACKER,
        "case \"at the cap\": the de-dup set must match the stream, got {}",
        emulator.remembered_event_ids("tracker-1")
    );

    // One batch past the cap drops exactly that many of the oldest records.
    let overflow: Vec<Value> = (0..MAX_BATCH)
        .map(|offset| {
            event(
                "click",
                offset as f64,
                json!({ "eventId": format!("overflow-{offset}") }),
            )
        })
        .collect();
    assert_accepted(
        "one batch past the cap",
        &emulator
            .post(
                "/events",
                &put_events_body("tracker-1", "session-1", overflow),
            )
            .await,
    );
    let after = emulator.stored_events("tracker-1");
    assert_eq!(
        after.len(),
        MAX_EVENTS_PER_TRACKER,
        "case \"one batch past the cap\": expected the stream to stay at its cap, got {}",
        after.len()
    );
    assert_eq!(
        after.first().map(|event| event.event_id.clone()),
        Some(format!("e-{MAX_BATCH}")),
        "case \"one batch past the cap\": expected the oldest {MAX_BATCH} records to be dropped, \
         got {:?}",
        after.first()
    );
    assert_eq!(
        after.last().map(|event| event.event_id.clone()),
        Some(format!("overflow-{}", MAX_BATCH - 1)),
        "case \"one batch past the cap\": expected the newest record last, got {:?}",
        after.last()
    );
    assert_eq!(
        emulator.remembered_event_ids("tracker-1"),
        MAX_EVENTS_PER_TRACKER,
        "case \"one batch past the cap\": the de-dup set must not outgrow the stream, got {}",
        emulator.remembered_event_ids("tracker-1")
    );

    // An id that fell out of retention is no longer de-duplicated: the tracker does not
    // hold that event any more, so sending it again is a new event.
    assert_accepted(
        "re-sending a dropped id",
        &emulator
            .post(
                "/events",
                &put_events_body(
                    "tracker-1",
                    "session-1",
                    vec![event("click", 1.0, json!({ "eventId": "e-0" }))],
                ),
            )
            .await,
    );
    let revived = emulator.stored_events("tracker-1");
    assert_eq!(
        revived.last().map(|event| event.event_id.clone()),
        Some("e-0".to_string()),
        "case \"re-sending a dropped id\": expected it ingested as a new event, got {:?}",
        revived.last()
    );
    assert_eq!(
        revived.len(),
        MAX_EVENTS_PER_TRACKER,
        "case \"re-sending a dropped id\": expected the stream to stay at its cap, got {}",
        revived.len()
    );
    // And an id still in retention is still de-duplicated.
    assert_accepted(
        "re-sending a retained id",
        &emulator
            .post(
                "/events",
                &put_events_body(
                    "tracker-1",
                    "session-1",
                    vec![event("click", 1.0, json!({ "eventId": "overflow-0" }))],
                ),
            )
            .await,
    );
    assert_eq!(
        emulator
            .stored_events("tracker-1")
            .iter()
            .filter(|event| event.event_id == "overflow-0")
            .count(),
        1,
        "case \"re-sending a retained id\": expected it held once"
    );
}

#[tokio::test]
async fn an_action_interaction_stream_retains_its_most_recent_records() {
    let emulator = Emulator::new();
    emulator.seed_tracker("tracker-1");

    let batches = MAX_ACTION_INTERACTIONS_PER_TRACKER / MAX_BATCH + 1;
    for batch in 0..batches {
        let interactions: Vec<Value> = (0..MAX_BATCH)
            .map(|offset| {
                let index = batch * MAX_BATCH + offset;
                interaction(
                    "action-1",
                    index as f64,
                    json!({ "eventId": format!("i-{index}") }),
                )
            })
            .collect();
        let case = format!("interaction batch {batch}");
        assert_accepted(
            &case,
            &emulator
                .post(
                    "/action-interactions",
                    &put_interactions_body("tracker-1", interactions),
                )
                .await,
        );
    }

    let stored = emulator.stored_interactions("tracker-1");
    assert_eq!(
        stored.len(),
        MAX_ACTION_INTERACTIONS_PER_TRACKER,
        "expected the stream to stay at its cap, got {}",
        stored.len()
    );
    assert_eq!(
        stored.first().map(|held| held.event_id.clone()),
        Some(format!("i-{MAX_BATCH}")),
        "expected the oldest {MAX_BATCH} records to be dropped, got {:?}",
        stored.first()
    );
    assert_eq!(
        emulator.remembered_interaction_event_ids("tracker-1"),
        MAX_ACTION_INTERACTIONS_PER_TRACKER,
        "the de-dup set must not outgrow the stream, got {}",
        emulator.remembered_interaction_event_ids("tracker-1")
    );
}

#[tokio::test]
async fn the_registry_refuses_a_tracker_past_its_capacity_and_keeps_serving_the_known_ones() {
    let emulator = Emulator::new();

    // No modelled operation creates a tracker, so the capacity bounds the registry seam
    // itself: fill it, then the next unknown id is refused.
    for index in 0..MAX_EVENT_TRACKERS {
        emulator.seed_tracker(&format!("tracker-{index}"));
    }
    assert!(
        emulator
            .engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_event_tracker("one-too-many", None)
            .is_none(),
        "case \"one tracker too many\": the registry must refuse past its capacity"
    );
    assert!(
        emulator
            .engine
            .state::<PersonalizeEventsState>()
            .lock()
            .register_dataset(ITEMS_DATASET)
            .is_some(),
        "case \"one tracker too many\": the dataset registry is bounded separately"
    );

    // An unknown tracking id is refused at the wire, and nothing is minted for it.
    let past_capacity = emulator
        .post(
            "/events",
            &put_events_body(
                "one-too-many",
                "session-1",
                vec![event("click", 1.0, json!({ "eventId": "e-1" }))],
            ),
        )
        .await;
    assert_refused(
        "one tracker too many",
        &past_capacity,
        400,
        "InvalidInputException",
    );
    assert!(
        emulator.stored_events("one-too-many").is_empty(),
        "case \"one tracker too many\": nothing may be persisted, got {:?}",
        emulator.stored_events("one-too-many")
    );

    // Every tracker the store already knows keeps ingesting.
    for tracking_id in ["tracker-0", &format!("tracker-{}", MAX_EVENT_TRACKERS - 1)] {
        assert_accepted(
            tracking_id,
            &emulator
                .post(
                    "/events",
                    &put_events_body(
                        tracking_id,
                        "session-1",
                        vec![event("click", 2.0, json!({ "eventId": "e-2" }))],
                    ),
                )
                .await,
        );
        assert_eq!(
            event_ids(&emulator.stored_events(tracking_id)),
            vec!["e-2".to_string()],
            "case {tracking_id:?}: expected its own event, got {:?}",
            event_ids(&emulator.stored_events(tracking_id))
        );
    }
}

#[tokio::test]
async fn a_dataset_collection_refuses_records_past_its_capacity() {
    let emulator = Emulator::new();
    emulator.seed_dataset(ITEMS_DATASET);

    // Fill it to one short of the cap: whole batches first, then the remainder.
    let short_of_cap = MAX_RECORDS_PER_DATASET_COLLECTION - 1;
    let mut stored_count = 0;
    while stored_count < short_of_cap {
        let size = MAX_BATCH.min(short_of_cap - stored_count);
        let items: Vec<Value> = (0..size)
            .map(|offset| json!({ "itemId": format!("item-{}", stored_count + offset) }))
            .collect();
        let case = format!("items {stored_count}..{}", stored_count + size);
        assert_accepted(
            &case,
            &emulator
                .post(
                    "/items",
                    &json!({ "datasetArn": ITEMS_DATASET, "items": items }),
                )
                .await,
        );
        stored_count += size;
    }
    assert_eq!(
        emulator
            .stored_records(ITEMS_DATASET, MetadataKind::Items)
            .len(),
        short_of_cap,
        "expected the collection one short of its cap"
    );

    // A batch that repeats ONE new id grows the collection by one, not by its length,
    // so it fits in the single free slot.
    let repeated: Vec<Value> = (0..MAX_BATCH)
        .map(|offset| {
            json!({ "itemId": "repeated", "properties": format!("{{\"n\":{offset}}}") })
        })
        .collect();
    assert_accepted(
        "a batch repeating one new id in the last free slot",
        &emulator
            .post(
                "/items",
                &json!({ "datasetArn": ITEMS_DATASET, "items": repeated }),
            )
            .await,
    );
    let stored = emulator.stored_records(ITEMS_DATASET, MetadataKind::Items);
    assert_eq!(
        stored.len(),
        MAX_RECORDS_PER_DATASET_COLLECTION,
        "case \"a batch repeating one new id in the last free slot\": expected exactly one new \
         record, got {}",
        stored.len()
    );
    assert_eq!(
        stored
            .get("repeated")
            .and_then(|record| record.properties.clone()),
        Some(json!({ "n": MAX_BATCH - 1 })),
        "case \"a batch repeating one new id in the last free slot\": expected the last \
         occurrence to be the record, got {:?}",
        stored.get("repeated")
    );

    let past_capacity = emulator
        .post(
            "/items",
            &json!({ "datasetArn": ITEMS_DATASET, "items": [{ "itemId": "one-too-many" }] }),
        )
        .await;
    assert_refused(
        "one record too many",
        &past_capacity,
        400,
        "InvalidInputException",
    );
    let stored = emulator.stored_records(ITEMS_DATASET, MetadataKind::Items);
    assert!(
        !stored.contains_key("one-too-many"),
        "case \"one record too many\": the refused record must not be stored"
    );
    assert_eq!(
        stored.len(),
        MAX_RECORDS_PER_DATASET_COLLECTION,
        "case \"one record too many\": the collection must not grow, got {}",
        stored.len()
    );

    // Replacing a record the collection already holds does not grow it, so it is
    // accepted at the cap.
    assert_accepted(
        "replacing a record at the cap",
        &emulator
            .post(
                "/items",
                &json!({
                    "datasetArn": ITEMS_DATASET,
                    "items": [{ "itemId": "item-0", "properties": r#"{"genre":"Comedy"}"# }],
                }),
            )
            .await,
    );
    let stored = emulator.stored_records(ITEMS_DATASET, MetadataKind::Items);
    assert_eq!(
        stored.len(),
        MAX_RECORDS_PER_DATASET_COLLECTION,
        "case \"replacing a record at the cap\": the collection must not grow, got {}",
        stored.len()
    );
    assert_eq!(
        stored.get("item-0").and_then(|record| record.properties.clone()),
        Some(json!({ "genre": "Comedy" })),
        "case \"replacing a record at the cap\": expected the new properties, got {:?}",
        stored.get("item-0")
    );
}

/// A generated `eventId` is unique across the WHOLE store, not merely within the stream
/// that de-duplicates on it: `Event$eventId` / `ActionInteraction$eventId` say Personalize
/// "generates a unique ID for the event", and nothing scopes that to one tracker.
///
/// The rows differ along the axes a generated id has to stay unique over: which tracker
/// ingests, which of the two streams it lands in, how many ids one call generates, and
/// how many calls there are. Every id is also checked against the member's one modelled
/// constraint, length 1..256.
#[tokio::test]
async fn a_generated_event_id_is_unique_across_trackers_and_both_streams() {
    /// (tracker, route, how many ids this call leaves to the service).
    const CALLS: [(&str, &str, usize); 8] = [
        ("tracker-1", "/events", 1),
        ("tracker-1", "/events", 3),
        ("tracker-1", "/action-interactions", 1),
        ("tracker-1", "/action-interactions", 10),
        ("tracker-2", "/events", 10),
        ("tracker-2", "/action-interactions", 2),
        ("tracker-3", "/events", 2),
        ("tracker-3", "/action-interactions", 1),
    ];

    let emulator = Emulator::new();
    for tracking_id in ["tracker-1", "tracker-2", "tracker-3"] {
        emulator.seed_tracker(tracking_id);
    }

    let mut expected_generated: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (tracking_id, route, count) in CALLS {
        let case = format!("{count} generated ids on {route} of {tracking_id}");
        let body = match route {
            "/events" => put_events_body(
                tracking_id,
                "session-1",
                (0..count)
                    .map(|index| event("click", 1_764_000_000.0 + index as f64, json!({})))
                    .collect(),
            ),
            _ => put_interactions_body(
                tracking_id,
                (0..count)
                    .map(|index| {
                        interaction("action-1", 1_764_000_000.0 + index as f64, json!({}))
                    })
                    .collect(),
            ),
        };
        assert_accepted(&case, &emulator.post(route, &body).await);

        let counts = expected_generated.entry(tracking_id).or_insert((0, 0));
        if route == "/events" {
            counts.0 += count;
        } else {
            counts.1 += count;
        }
    }

    // Collect every id the service minted, across both streams of all three trackers.
    let mut minted: Vec<String> = Vec::new();
    for (tracking_id, (events, interactions)) in &expected_generated {
        let stored = emulator.stored_events(tracking_id);
        assert_eq!(
            stored.len(),
            *events,
            "case {tracking_id:?}: expected {events} stored events, got {:?}",
            event_ids(&stored)
        );
        minted.extend(event_ids(&stored));

        let held = emulator.stored_interactions(tracking_id);
        assert_eq!(
            held.len(),
            *interactions,
            "case {tracking_id:?}: expected {interactions} stored interactions, got {:?}",
            held.iter()
                .map(|interaction| interaction.event_id.clone())
                .collect::<Vec<_>>()
        );
        minted.extend(held.iter().map(|interaction| interaction.event_id.clone()));
    }

    let expected_total: usize = CALLS.iter().map(|(_, _, count)| count).sum();
    assert_eq!(
        minted.len(),
        expected_total,
        "case \"every call\": expected {expected_total} generated ids in all, got {}",
        minted.len()
    );
    assert!(
        minted.iter().all(|id| !id.is_empty() && id.len() <= 256),
        "case \"every call\": a generated event id must satisfy length 1..256, got {minted:?}"
    );
    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "case \"every call\": every generated event id must differ from every other, \
         across trackers and across both streams, got {minted:?}"
    );
}

// ---------------------------------------------------------------------------
// A dataset's TYPE decides which metadata put it serves.
// ---------------------------------------------------------------------------

/// A dataset ARN naming the type `segment`, in the dataset group `group`: the resource of
/// a Personalize dataset ARN is `dataset/<dataset group name>/<DATASET_TYPE>`, so the
/// type is the last segment.
fn dataset_arn_of_type(group: &str, segment: &str) -> String {
    format!("arn:aws:personalize:us-east-1:123456789012:dataset/{group}/{segment}")
}

/// A one-record metadata put for `case`, with the record named the way that operation
/// names its records.
fn metadata_body(case: &MetadataCase, dataset_arn: &str, id: &str) -> Value {
    json!({ "datasetArn": dataset_arn, case.collection: [{ case.id_member: id }] })
}

/// Every ARN type segment the table below addresses each metadata put with, and the type
/// the derivation must read out of it.
///
/// The published values come from `DatasetType::ALL`, whose doc comment cites the Amazon
/// Personalize control plane's `CreateDatasetRequest$datasetType` documentation ("One of
/// the following (case insensitive) values: Interactions, Items, Users, Actions,
/// Action_Interactions"); an ARN renders them upper-cased. The extra rows are the casings
/// that documentation makes equivalent, and segments that name no published type at all —
/// including `INTERACTIONS` and `ACTION_INTERACTIONS`, which the bootstrap issue never
/// cited.
fn type_segments() -> Vec<(String, Option<DatasetType>)> {
    let mut segments: Vec<(String, Option<DatasetType>)> = DatasetType::ALL
        .into_iter()
        .map(|dataset_type| {
            (
                dataset_type.as_str().to_ascii_uppercase(),
                Some(dataset_type),
            )
        })
        .collect();
    segments.extend([
        // The documented spelling itself, plus a lower-cased and a mixed-case one: the
        // match is case insensitive.
        (
            "Action_Interactions".to_string(),
            Some(DatasetType::ActionInteractions),
        ),
        ("users".to_string(), Some(DatasetType::Users)),
        ("iTeMs".to_string(), Some(DatasetType::Items)),
        // Segments naming no published type: a near miss in either direction, the
        // compound value without its underscore, a name, and an empty segment.
        ("ITEM".to_string(), None),
        ("INTERACTION".to_string(), None),
        ("ACTIONINTERACTIONS".to_string(), None),
        ("my-dataset".to_string(), None),
        (String::new(), None),
    ]);
    segments
}

/// `PutItems`, `PutUsers` and `PutActions` each serve only a dataset of the type their
/// records ARE — the type the dataset ARN's last segment names. A put against a dataset
/// of any other published type, or of no published type at all, is refused with the
/// modelled `InvalidInputException` (400) and stores nothing.
#[tokio::test]
async fn a_metadata_put_serves_only_a_dataset_whose_type_matches_its_records() {
    let emulator = Emulator::new();

    for (index, (segment, expected_type)) in type_segments().into_iter().enumerate() {
        for case in &METADATA_CASES {
            // One dataset per (segment, operation) pair, so no row can be satisfied by
            // what another row stored.
            let group = format!("type-table-{index}-{}", case.collection);
            let dataset_arn = dataset_arn_of_type(&group, &segment);
            emulator.seed_dataset(&dataset_arn);
            assert_eq!(
                emulator.dataset_type(&dataset_arn),
                expected_type,
                "case {dataset_arn:?}: expected the registry to derive {expected_type:?} from \
                 the ARN, got {:?}",
                emulator.dataset_type(&dataset_arn)
            );

            let records_type = case.kind.dataset_type();
            let case_label = format!("{} into a {segment:?} dataset", case.route);
            let reply = emulator
                .post(case.route, &metadata_body(case, &dataset_arn, "record-1"))
                .await;
            let stored = emulator.stored_records(&dataset_arn, case.kind);

            if expected_type == Some(records_type) {
                assert_accepted(&case_label, &reply);
                assert_eq!(
                    stored.keys().cloned().collect::<Vec<_>>(),
                    vec!["record-1".to_string()],
                    "case {case_label:?}: expected exactly the record it carried, got {:?}",
                    stored.keys().collect::<Vec<_>>()
                );
            } else {
                assert_refused(&case_label, &reply, 400, "InvalidInputException");
                assert!(
                    stored.is_empty(),
                    "case {case_label:?}: a refused put must store nothing, got {:?}",
                    stored.keys().collect::<Vec<_>>()
                );
                // The refusal names the dataset the caller addressed and the type the
                // operation serves, whatever type (or non-type) the ARN names.
                let expected = format!(
                    "{dataset_arn} is not a valid {} dataset.",
                    records_type.as_str().to_ascii_uppercase()
                );
                assert!(
                    reply.body.contains(&expected),
                    "case {case_label:?}: expected the message {expected:?}, got {:?}",
                    reply.body
                );
            }
        }
    }
}

/// The issue's own step, and the put that IS served: a users put addressed at the
/// bootstrap issue's `.../demo/ITEMS` dataset is refused and stores nothing, while the
/// same records go in through `.../demo/USERS` — so the refusal is about the dataset's
/// type and not about the records.
#[tokio::test]
async fn a_users_put_against_the_items_dataset_is_refused_and_the_users_dataset_takes_it() {
    let emulator = Emulator::new();
    emulator.seed_dataset(ITEMS_DATASET);
    emulator.seed_dataset(USERS_DATASET);

    let users = json!([{ "userId": "user-1" }]);
    let refused = emulator
        .post(
            "/users",
            &json!({ "datasetArn": ITEMS_DATASET, "users": users }),
        )
        .await;
    assert_refused(
        "PutUsers into the demo ITEMS dataset",
        &refused,
        400,
        "InvalidInputException",
    );
    for (dataset_arn, kind) in [
        (ITEMS_DATASET, MetadataKind::Users),
        (ITEMS_DATASET, MetadataKind::Items),
    ] {
        assert!(
            emulator.stored_records(dataset_arn, kind).is_empty(),
            "case \"PutUsers into the demo ITEMS dataset\": nothing may be stored in {kind:?}, \
             got {:?}",
            emulator.stored_records(dataset_arn, kind)
        );
    }

    let accepted = emulator
        .post(
            "/users",
            &json!({ "datasetArn": USERS_DATASET, "users": users }),
        )
        .await;
    assert_accepted("PutUsers into the demo USERS dataset", &accepted);
    assert_eq!(
        emulator
            .stored_records(USERS_DATASET, MetadataKind::Users)
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["user-1".to_string()],
        "case \"PutUsers into the demo USERS dataset\": expected the user it carried, got {:?}",
        emulator.stored_records(USERS_DATASET, MetadataKind::Users)
    );
}
