// SPDX-License-Identifier: CC0-1.0

//! How the two ingestion puts refuse a `trackingId` they cannot ingest through.
//!
//! The rule under test, in one sentence: an ingestion put whose `trackingId` names no
//! target this service can ingest the requested stream through refuses the call as an
//! INPUT fault — HTTP 400 in the `InvalidInputException` class, never a not-found — with
//! the wording Amazon Personalize uses for that stream: `PutEvents` states the account
//! the call is authenticated to and `PutActionInteractions` states that action
//! interactions are not supported, and neither echoes the id the caller submitted.
//! Registering the tracker is the only difference the identical call needs to be
//! ingested.
//!
//! Every call goes over the wire through the same [`MultiServiceRouter`] the production
//! emulator mounts, so what is asserted is what a client sees: the status, the
//! `x-amzn-errortype` header, the body's `__type` and its `message`. What a served call
//! persisted is read back through the shared engine's store — the same store the handlers
//! write — because every operation of this service answers an empty body.
//!
//! The tests are tables. The rows differ along what the rule generalises over: the
//! operation (and so the stream), and the FORM of the tracking id — words, digits, a
//! uuid, mixed case with spaces, non-ASCII, and both bounds of the member's modelled
//! `@length 1..256`. The uuid-shaped row stands in for the id class the issue's recording
//! carried, with a different value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state::{
    ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState as PersonalizeEventsState,
    ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE,
};
use service_com_amazonaws_personalizeevents_amazonpersonalizeevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for; the router resolves the service from the
/// credential scope.
const REGION: &str = "us-east-1";

/// The sigv4 signing name from the model (`smithy_query(query="service_info")`: sigv4Name
/// `personalize`).
const SIGNING_NAME: &str = "personalize";

/// The one modelled client-input error of both ingestion puts (`smithy_query(
/// operation="PutEvents,PutActionInteractions")`: `InvalidInputException`, httpError 400).
const INVALID_INPUT: &str = "InvalidInputException";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The `x-amzn-errortype` header, or the empty string when the answer carries none.
    error_type: String,
    /// The `__type` of the error body, or the empty string for a body carrying none.
    code: String,
    /// The `message` of the error body, or the empty string for a body carrying none.
    message: String,
    /// The raw body; every served call of this service answers an empty one.
    body: String,
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router. The engine is kept so a test can seed the registry the
/// control plane would fill (`CreateEventTracker` lives in the `personalize` service) and
/// read back what ingestion persisted.
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
            error_type,
            code: field("__type"),
            message: field("message"),
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

    /// Mark a registered tracker busy, the way an import job holding it would.
    fn mark_tracker_in_use(&self, tracking_id: &str) {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .event_tracker_mut(tracking_id)
            .unwrap_or_else(|| panic!("the tracker {tracking_id} is registered"))
            .in_use = true;
    }

    /// How many records the tracker's item-interaction stream holds.
    fn stored_events(&self, tracking_id: &str) -> usize {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .event_trackers
            .get(tracking_id)
            .map_or(0, |tracker| tracker.events.len())
    }

    /// How many records the tracker's action-interaction stream holds.
    fn stored_interactions(&self, tracking_id: &str) -> usize {
        self.engine
            .state::<PersonalizeEventsState>()
            .lock()
            .event_trackers
            .get(tracking_id)
            .map_or(0, |tracker| tracker.action_interactions.len())
    }
}

/// The tracking-id forms every row of both tables is driven with: words, digits, a
/// uuid-shaped id, mixed case with spaces, non-ASCII, and each bound of the member's
/// modelled `@length 1..256` (`smithy_query(operation="PutEvents")`: `trackingId`
/// StringType, length 1..256).
fn tracking_id_forms() -> Vec<(&'static str, String)> {
    vec![
        ("words", "unknown-tracker".to_string()),
        ("digits", "1234567890".to_string()),
        ("uuid", "11111111-2222-4333-8444-555555555555".to_string()),
        ("mixed case with spaces", "Tracker Two Words".to_string()),
        ("non-ASCII", "trücker-ünïcödé".to_string()),
        ("the length minimum", "a".to_string()),
        ("the length maximum", "t".repeat(256)),
    ]
}

/// Which stream an ingestion put writes, as a test row addresses it: the route, and the
/// one-record body that put takes for `tracking_id`.
#[derive(Debug, Clone, Copy)]
enum Stream {
    Events,
    ActionInteractions,
}

impl Stream {
    const ALL: [Stream; 2] = [Stream::Events, Stream::ActionInteractions];

    fn route(self) -> &'static str {
        match self {
            Stream::Events => "/events",
            Stream::ActionInteractions => "/action-interactions",
        }
    }

    /// A minimal, otherwise valid one-record request for this stream.
    fn body(self, tracking_id: &str) -> Value {
        match self {
            Stream::Events => json!({
                "trackingId": tracking_id,
                "sessionId": "session-1",
                "eventList": [{ "eventType": "click", "sentAt": 1_764_000_000.0 }],
            }),
            Stream::ActionInteractions => json!({
                "trackingId": tracking_id,
                "actionInteractions": [{
                    "actionId": "action-1",
                    "sessionId": "session-1",
                    "timestamp": 1_764_000_000.0,
                    "eventType": "Taken",
                }],
            }),
        }
    }

    /// The same request, additionally carrying a `properties` map using one of the keys
    /// the two event streams reserve — so the request is wrong about its target AND about
    /// a record.
    fn body_with_a_reserved_property(self, tracking_id: &str) -> Value {
        let mut body = self.body(tracking_id);
        let records = match self {
            Stream::Events => "eventList",
            Stream::ActionInteractions => "actionInteractions",
        };
        body[records][0]["properties"] = json!(r#"{"sessionId":"x"}"#);
        body
    }

    /// What this stream's put answers for a tracking id it cannot ingest through.
    fn unusable_tracking_id_message(self) -> String {
        match self {
            // The account the call is authenticated to is the one account this emulator
            // serves, so the message's one variable part is read off the single source of
            // truth for it rather than spelled out here.
            Stream::Events => format!(
                "Invalid trackingId in account {}",
                engine::emulator_account_id()
            ),
            Stream::ActionInteractions => ACTION_INTERACTIONS_NOT_SUPPORTED_MESSAGE.to_string(),
        }
    }

    /// How many records `tracking_id`'s stream of this kind holds.
    fn stored(self, emulator: &Emulator, tracking_id: &str) -> usize {
        match self {
            Stream::Events => emulator.stored_events(tracking_id),
            Stream::ActionInteractions => emulator.stored_interactions(tracking_id),
        }
    }
}

/// RULE: a `trackingId` naming no target this service can ingest the requested stream
/// through is refused as an input fault — 400 `InvalidInputException` with that stream's
/// AWS wording — and persists nothing; registering the tracker is the only difference the
/// identical call needs to be ingested.
#[tokio::test]
async fn an_unusable_tracking_id_is_an_input_fault_worded_the_way_aws_words_it() {
    for stream in Stream::ALL {
        for (form, tracking_id) in tracking_id_forms() {
            let case = format!("{} trackingId {form}", stream.route());
            // A fresh emulator per row, so the row's own call is all its store holds.
            let emulator = Emulator::new();

            let refused = emulator.post(stream.route(), &stream.body(&tracking_id)).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400, got {} / {:?} {:?}",
                refused.status, refused.error_type, refused.body
            );
            assert_eq!(
                refused.error_type, INVALID_INPUT,
                "case {case:?}: expected the error type {INVALID_INPUT:?}, got {:?}",
                refused.error_type
            );
            assert_eq!(
                refused.code, INVALID_INPUT,
                "case {case:?}: expected the body code {INVALID_INPUT:?}, got {:?}",
                refused.code
            );
            // Equality is what rules out an echo of the submitted id: any id spliced into
            // the sentence would make it differ from the one AWS words.
            let expected = stream.unusable_tracking_id_message();
            assert_eq!(
                refused.message, expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                refused.message
            );
            assert_eq!(
                stream.stored(&emulator, &tracking_id),
                0,
                "case {case:?}: a refused call must persist nothing, got {} record(s)",
                stream.stored(&emulator, &tracking_id)
            );

            // The positive control: the identical call against a registered tracker is
            // served and its record is in that tracker's own stream.
            emulator.seed_tracker(&tracking_id);
            let served = emulator.post(stream.route(), &stream.body(&tracking_id)).await;
            assert_eq!(
                served.status, 200,
                "case {case:?}: expected a registered tracker to be served with HTTP 200, \
                 got {} / {:?} {:?}",
                served.status, served.error_type, served.body
            );
            assert!(
                served.body.is_empty(),
                "case {case:?}: expected an empty body, got {:?}",
                served.body
            );
            assert_eq!(
                stream.stored(&emulator, &tracking_id),
                1,
                "case {case:?}: expected the one record it carried to be persisted, got {}",
                stream.stored(&emulator, &tracking_id)
            );
        }
    }
}

/// RULE: the target is resolved first, so a request wrong about BOTH its tracking id and
/// a record it carries answers for the tracking id — the stronger, more specific fault —
/// with the same wording and status as one wrong only about the tracking id.
#[tokio::test]
async fn a_request_wrong_about_its_tracking_id_and_a_record_answers_for_the_tracking_id() {
    for stream in Stream::ALL {
        for (form, tracking_id) in tracking_id_forms() {
            let case = format!("{} trackingId {form} plus a reserved properties key", stream.route());
            let emulator = Emulator::new();

            let reply = emulator
                .post(
                    stream.route(),
                    &stream.body_with_a_reserved_property(&tracking_id),
                )
                .await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} / {:?} {:?}",
                reply.status, reply.error_type, reply.body
            );
            let expected = stream.unusable_tracking_id_message();
            assert_eq!(
                reply.message, expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                reply.message
            );
        }
    }
}

/// RULE: a tracking id that DOES resolve is not an input fault, so the refusals above are
/// reached only through resolution — a registered-but-busy tracker still answers the
/// modelled `ResourceInUseException` (409) for action interactions, and `PutEvents`, which
/// models no such error, still ingests.
#[tokio::test]
async fn a_resolvable_tracker_answers_its_own_state_and_not_the_input_fault() {
    let emulator = Emulator::new();
    let tracking_id = "busy-tracker";
    emulator.seed_tracker(tracking_id);
    emulator.mark_tracker_in_use(tracking_id);

    let interactions = emulator
        .post(
            Stream::ActionInteractions.route(),
            &Stream::ActionInteractions.body(tracking_id),
        )
        .await;
    assert_eq!(
        interactions.status, 409,
        "case \"in-use tracker /action-interactions\": expected HTTP 409, got {} / {:?} {:?}",
        interactions.status, interactions.error_type, interactions.body
    );
    assert_eq!(
        interactions.error_type, "ResourceInUseException",
        "case \"in-use tracker /action-interactions\": expected the modelled in-use error, \
         got {:?}",
        interactions.error_type
    );

    let events = emulator
        .post(Stream::Events.route(), &Stream::Events.body(tracking_id))
        .await;
    assert_eq!(
        events.status, 200,
        "case \"in-use tracker /events\": expected HTTP 200, got {} / {:?} {:?}",
        events.status, events.error_type, events.body
    );
    assert_eq!(
        emulator.stored_events(tracking_id),
        1,
        "case \"in-use tracker /events\": expected the event to be persisted, got {}",
        emulator.stored_events(tracking_id)
    );
}

/// RULE: the account the `PutEvents` refusal names is the account this emulator serves —
/// a 12-digit AWS account id — and not a value taken from the request.
#[tokio::test]
async fn the_put_events_refusal_names_the_emulators_own_account() {
    let account = engine::emulator_account_id();
    assert!(
        account.len() == 12 && account.chars().all(|digit| digit.is_ascii_digit()),
        "case \"the emulator's account\": expected a 12-digit account id, got {account:?}"
    );

    let emulator = Emulator::new();
    // Two requests differing in everything the message could wrongly be derived from:
    // the tracking id, the session and the event.
    let rows = [
        (
            "first request",
            json!({
                "trackingId": "one-tracker",
                "sessionId": "session-1",
                "eventList": [{ "eventType": "click", "sentAt": 1_764_000_000.0 }],
            }),
        ),
        (
            "second request",
            json!({
                "trackingId": "another-tracker",
                "sessionId": "session-2",
                "eventList": [{ "eventType": "watch", "sentAt": 0.0 }],
            }),
        ),
    ];
    for (case, body) in rows {
        let reply = emulator.post("/events", &body).await;
        assert_eq!(
            reply.message,
            format!("Invalid trackingId in account {account}"),
            "case {case:?}: expected the account-scoped refusal, got {:?}",
            reply.message
        );
    }
}
