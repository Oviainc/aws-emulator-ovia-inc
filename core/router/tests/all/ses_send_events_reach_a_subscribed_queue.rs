// SPDX-License-Identifier: CC0-1.0

//! The email events a SESv2 send produces reach the queue subscribed to the configuration
//! set's SNS event destination — through the WHOLE mounted service set, the one
//! `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test, in three parts:
//!
//! * **what a send produces**: an accepted message produces one `Send` event, plus one
//!   event per reaction its recipients have to it — `Delivery` for a recipient that took
//!   the message, `Bounce` for one it failed for, `Complaint` for one that reported it.
//! * **where it goes**: every event destination of the set the message named that is
//!   `Enabled` and whose `MatchingEventTypes` names the event's type receives one
//!   notification of it, and every other destination receives nothing.
//! * **what a hard bounce leaves behind**: the recipient is added to the account-level
//!   suppression list, with `Reason` `BOUNCE` and the `MessageId` of the message that
//!   bounced, so the NEXT send to that address is not handed on at all: it is still
//!   accepted, with a `MessageId` of its own, and produces a `Bounce` whose
//!   `bounceSubType` is `OnAccountSuppressionList` and whose `diagnosticCode` is the
//!   documented suppression-list sentence.
//!
//! Three services have to agree for that to hold — SESv2 publishes, SNS fans out, SQS
//! holds what arrived — and the publish SESv2 makes is an in-process call addressed the way
//! a client's own `sns`-scoped call is addressed, so WHICH mounted model claims it is only
//! decidable with every service mounted. The crate-local suite
//! `services/service-com-amazonaws-sesv2-simpleemailservice-v2/tests/all/send_suppression_from_feedback.rs`
//! pins what the suppression list is written from over a table of recipients and account
//! preferences; what only this suite can reach is the published event itself, over the
//! production service set.
//!
//! The rows of the second test differ along what the rule generalises over: whether the
//! destination is on, which event types it matches (one type, a pair, every modelled type)
//! and which reaction the addressed recipient has.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    drain, drain_until, given_topic_with_subscriber, Grant,
};

/// The Region the whole wiring of these cases lives in.
const REGION: &str = "us-west-2";

/// The domain identity that authorises every send below, and the sender it covers.
const SENDER_DOMAIN: &str = "senders.example.com";
const SENDER: &str = "sender@senders.example.com";

/// The configuration set the sends name, and the event destination hung off it.
const SET: &str = "event-publication";
const DESTINATION: &str = "events";

/// Every value of the SESv2 `EventType` enum, in the model's own order
/// (`smithy_query(query="shape",shape="EventType")`), for the row that matches the whole
/// published surface rather than the types one scenario happens to watch.
const EVERY_EVENT_TYPE: &[&str] = &[
    "SEND",
    "REJECT",
    "BOUNCE",
    "COMPLAINT",
    "DELIVERY",
    "OPEN",
    "CLICK",
    "RENDERING_FAILURE",
    "DELIVERY_DELAY",
    "SUBSCRIPTION",
];

/// The `diagnosticCode` a bounce carries when the account's suppression list, rather than a
/// mail server, is why the message was not sent. Written out here, so the suite pins the
/// sentence rather than reading it back from the code under test.
const ON_SUPPRESSION_LIST_DIAGNOSTIC_CODE: &str =
    "Amazon SES did not send the message to this address because it is on the suppression \
     list for your account. For more information about removing addresses from the \
     suppression list, see the Amazon SES Developer Guide at \
     https://docs.aws.amazon.com/ses/latest/dg/sending-email-suppression-list.html";

/// The whole emulator over one engine, exactly as the binary assembles it, with the engine
/// kept so a case can bring the verification transition the clock owes an identity into the
/// past instead of waiting out its span.
fn router() -> (MultiServiceRouter, std::sync::Arc<aws_emulator_router::InMemoryEngine>) {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(std::sync::Arc::clone(&engine));
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    (router, engine)
}

/// One answered SESv2 call: the status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message an error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string at `pointer`, or `""` when the body carries nothing there.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One SESv2 call: `restJson1` at the `email` endpoint, in SESv2's own `/v2/email/…` path
/// space, routed by a SigV4 credential scope naming the signing name the SES family shares.
async fn sesv2(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(value.to_string()),
        None => bytes::Bytes::new(),
    };
    let host = format!("email.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ses/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// The same call, which must answer 200.
async fn sesv2_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let answered = sesv2(router, method, path, body).await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from SESv2 {method} {path}, got {} {} ({})",
        answered.status,
        answered.error_code(),
        answered.message()
    );
    answered
}

/// A verified sending domain, so every send below is authorised.
async fn given_verified_sender(
    router: &MultiServiceRouter,
    engine: &aws_emulator_router::InMemoryEngine,
    case: &str,
) {
    sesv2_ok(
        router,
        case,
        "POST",
        "/v2/email/identities",
        Some(json!({"EmailIdentity": SENDER_DOMAIN})),
    )
    .await;
    let state = engine.state::<service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State>();
    let mut store = state.lock();
    let past = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .expect("an instant before now");
    for record in store.identities.values_mut() {
        record.verification_due_at = past;
    }
    store.settle(std::time::SystemTime::now());
}

/// The configuration set `SET` with one SNS event destination publishing to `topic`.
async fn given_configuration_set(
    router: &MultiServiceRouter,
    case: &str,
    topic: &str,
    enabled: bool,
    matching: &[&str],
) {
    sesv2_ok(
        router,
        case,
        "POST",
        "/v2/email/configuration-sets",
        Some(json!({"ConfigurationSetName": SET})),
    )
    .await;
    sesv2_ok(
        router,
        case,
        "POST",
        &format!("/v2/email/configuration-sets/{SET}/event-destinations"),
        Some(json!({
            "EventDestinationName": DESTINATION,
            "EventDestination": {
                "Enabled": enabled,
                "MatchingEventTypes": matching,
                "SnsDestination": {"TopicArn": topic},
            },
        })),
    )
    .await;
}

/// `SendEmail` to `recipient` through the configuration set, answering the `MessageId`.
async fn send(router: &MultiServiceRouter, case: &str, recipient: &str) -> String {
    let answered = sesv2_ok(
        router,
        case,
        "POST",
        "/v2/email/outbound-emails",
        Some(json!({
            "FromEmailAddress": SENDER,
            "ConfigurationSetName": SET,
            "Destination": {"ToAddresses": [recipient]},
            "Content": {"Simple": {
                "Subject": {"Data": "a message under test"},
                "Body": {"Text": {"Data": "hello"}},
            }},
        })),
    )
    .await;
    let message_id = answered.text("/MessageId");
    assert!(
        !message_id.is_empty(),
        "case {case:?}: an accepted send must answer a MessageId, got {}",
        answered.body
    );
    message_id
}

/// The one event of type `event_type` among `events`, which must be there exactly once.
fn one_event<'a>(case: &str, events: &'a [Value], event_type: &str) -> &'a Value {
    let matched: Vec<&Value> = events
        .iter()
        .filter(|event| event["eventType"].as_str() == Some(event_type))
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "case {case:?}: expected exactly one {event_type} event, got {events:?}"
    );
    matched[0]
}

/// The event types among `events`, in the order they arrived.
fn event_types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["eventType"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_hard_bounce_suppresses_the_address_and_the_next_send_bounces_on_the_list() {
    let case = "a send to a mailbox the simulator does not define, then one to the same address";
    let (router, engine) = router();
    let (topic, queue) =
        given_topic_with_subscriber(&router, case, REGION, "ses-send-events", Grant::Unscoped)
            .await;
    given_verified_sender(&router, &engine, case).await;
    given_configuration_set(&router, case, &topic, true, &["SEND", "BOUNCE"]).await;

    // A mailbox of the simulator's domain that the simulator does not define, which is
    // therefore no mailbox at all.
    let target = "c4f0-not-a-mailbox@simulator.amazonses.com";
    let suppression_path = format!("/v2/email/suppression/addresses/{target}");

    // Before the send the address is on no suppression list, and the read says so.
    let before = sesv2(&router, "GET", &suppression_path, None).await;
    assert_eq!(
        before.status,
        404,
        "case {case:?}: expected 404 before the address is suppressed, got {} ({})",
        before.status,
        before.message()
    );
    assert_eq!(
        before.message(),
        format!("Email address {target} does not exist on your suppression list."),
        "case {case:?}: expected the documented not-found sentence, got {:?}",
        before.message()
    );

    // The first send is accepted, and the hard bounce it produces reaches the queue
    // beside the acceptance.
    let first = send(&router, case, target).await;
    let events = drain_until(&router, case, REGION, &queue, 2).await;
    assert_eq!(
        events.len(),
        2,
        "case {case:?}: the send and the bounce it produced must both be published, got {:?}",
        event_types(&events)
    );
    let sent = one_event(case, &events, "Send");
    assert_eq!(
        sent["mail"]["messageId"].as_str(),
        Some(first.as_str()),
        "case {case:?}: the Send event must name the message the send answered, got {sent}"
    );
    assert_eq!(
        sent["mail"]["source"].as_str(),
        Some(SENDER),
        "case {case:?}: the Send event must name the sender, got {sent}"
    );
    assert_eq!(
        sent["mail"]["destination"].as_array().map(Vec::as_slice),
        Some([json!(target)].as_slice()),
        "case {case:?}: the Send event must name the recipients, got {sent}"
    );
    let bounced = one_event(case, &events, "Bounce");
    assert_eq!(
        bounced["mail"]["messageId"].as_str(),
        Some(first.as_str()),
        "case {case:?}: the Bounce event must name the message that bounced, got {bounced}"
    );
    assert_eq!(
        bounced["bounce"]["bounceType"].as_str(),
        Some("Permanent"),
        "case {case:?}: an unknown mailbox is a permanent failure, got {bounced}"
    );
    assert_eq!(
        bounced["bounce"]["bounceSubType"].as_str(),
        Some("General"),
        "case {case:?}: a mail server's own refusal is a General bounce, got {bounced}"
    );
    assert_eq!(
        bounced["bounce"]["bouncedRecipients"][0]["emailAddress"].as_str(),
        Some(target),
        "case {case:?}: the bounce must name the recipient it failed for, got {bounced}"
    );
    assert!(
        !bounced["bounce"]["bouncedRecipients"][0]["diagnosticCode"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "case {case:?}: a bounce the receiving side produced must carry what it said, got \
         {bounced}"
    );

    // The hard bounce put the address on the account-level suppression list, naming the
    // message that bounced.
    let entry = sesv2_ok(&router, case, "GET", &suppression_path, None).await;
    assert_eq!(
        entry.text("/SuppressedDestination/Reason"),
        "BOUNCE",
        "case {case:?}: a hard bounce suppresses for BOUNCE, got {}",
        entry.body
    );
    assert_eq!(
        entry.text("/SuppressedDestination/Attributes/MessageId"),
        first,
        "case {case:?}: the entry must name the message that caused it, got {}",
        entry.body
    );

    // The second send to the now-suppressed address is still accepted, with a MessageId of
    // its own, and is never handed on: it bounces on the list.
    let second = send(&router, case, target).await;
    assert_ne!(
        second, first,
        "case {case:?}: every accepted message gets a unique MessageId"
    );
    let events = drain_until(&router, case, REGION, &queue, 2).await;
    assert_eq!(
        events.len(),
        2,
        "case {case:?}: the second send and its suppression-list bounce must both be \
         published, got {:?}",
        event_types(&events)
    );
    assert_eq!(
        one_event(case, &events, "Send")["mail"]["messageId"].as_str(),
        Some(second.as_str()),
        "case {case:?}: the Send event must name the second message, got {events:?}"
    );
    let suppressed = one_event(case, &events, "Bounce");
    assert_eq!(
        suppressed["mail"]["messageId"].as_str(),
        Some(second.as_str()),
        "case {case:?}: the Bounce event must name the message that was not sent, got \
         {suppressed}"
    );
    assert_eq!(
        suppressed["bounce"]["bounceSubType"].as_str(),
        Some("OnAccountSuppressionList"),
        "case {case:?}: the suppression list is why this message was not sent, got \
         {suppressed}"
    );
    assert_eq!(
        suppressed["bounce"]["bouncedRecipients"][0]["diagnosticCode"].as_str(),
        Some(ON_SUPPRESSION_LIST_DIAGNOSTIC_CODE),
        "case {case:?}: expected the documented suppression-list diagnostic, got \
         {suppressed}"
    );

    // The entry is left as the hard bounce wrote it: a send SES did not attempt learned
    // nothing new about the address.
    let entry = sesv2_ok(&router, case, "GET", &suppression_path, None).await;
    assert_eq!(
        entry.text("/SuppressedDestination/Attributes/MessageId"),
        first,
        "case {case:?}: a suppressed send must not rewrite the entry, got {}",
        entry.body
    );

    // The control: the simulator's accepting mailbox produces exactly a Send event for a
    // destination that matches SEND and BOUNCE, because nothing bounced.
    let control = send(&router, case, "success@simulator.amazonses.com").await;
    let events = drain_until(&router, case, REGION, &queue, 1).await;
    assert_eq!(
        event_types(&events),
        vec!["Send".to_string()],
        "case {case:?}: a message the recipient took produces no bounce, got {events:?}"
    );
    assert_eq!(
        events[0]["mail"]["messageId"].as_str(),
        Some(control.as_str()),
        "case {case:?}: the Send event must name the control message, got {events:?}"
    );
    assert!(
        drain(&router, case, REGION, &queue).await.is_empty(),
        "case {case:?}: nothing else may be published for these sends"
    );
}

/// The MIME document a raw send carries, with the headers in the order a caller wrote
/// them: the envelope headers a simple message would also carry, a header of the caller's
/// own, and the two a MIME reader needs.
const RAW_MESSAGE: &str = "From: sender@senders.example.com\r\n\
     To: success@simulator.amazonses.com\r\n\
     Subject: a raw message under test\r\n\
     X-Campaign-Id: spring-2026\r\n\
     MIME-Version: 1.0\r\n\
     Content-Type: text/plain; charset=UTF-8\r\n\
     \r\n\
     hello\r\n";

/// The headers [`RAW_MESSAGE`] is written with, in that order.
const RAW_MESSAGE_HEADERS: &[(&str, &str)] = &[
    ("From", "sender@senders.example.com"),
    ("To", "success@simulator.amazonses.com"),
    ("Subject", "a raw message under test"),
    ("X-Campaign-Id", "spring-2026"),
    ("MIME-Version", "1.0"),
    ("Content-Type", "text/plain; charset=UTF-8"),
];

#[tokio::test]
async fn the_event_of_a_raw_send_reports_the_headers_the_message_carried() {
    let case = "a message sent as a raw MIME document";
    let (router, engine) = router();
    let (topic, queue) =
        given_topic_with_subscriber(&router, case, REGION, "ses-send-events", Grant::Unscoped)
            .await;
    given_verified_sender(&router, &engine, case).await;
    given_configuration_set(&router, case, &topic, true, &["SEND"]).await;

    let answered = sesv2_ok(
        &router,
        case,
        "POST",
        "/v2/email/outbound-emails",
        Some(json!({
            "FromEmailAddress": SENDER,
            "ConfigurationSetName": SET,
            "Destination": {"ToAddresses": ["success@simulator.amazonses.com"]},
            "Content": {"Raw": {"Data": aws_smithy_types::base64::encode(RAW_MESSAGE)}},
        })),
    )
    .await;
    let message_id = answered.text("/MessageId");
    assert!(
        !message_id.is_empty(),
        "case {case:?}: an accepted raw send must answer a MessageId, got {}",
        answered.body
    );

    let events = drain_until(&router, case, REGION, &queue, 1).await;
    assert_eq!(
        event_types(&events),
        vec!["Send".to_string()],
        "case {case:?}: the destination matches SEND alone, got {events:?}"
    );
    let published: Vec<(String, String)> = events[0]["mail"]["headers"]
        .as_array()
        .map(|headers| {
            headers
                .iter()
                .map(|header| {
                    (
                        header["name"].as_str().unwrap_or_default().to_string(),
                        header["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let written: Vec<(String, String)> = RAW_MESSAGE_HEADERS
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    assert_eq!(
        published, written,
        "case {case:?}: the event must report the headers the document carried, in the order \
         and the case they were written, got {}",
        events[0]["mail"]
    );
    assert_eq!(
        events[0]["mail"]["commonHeaders"]["subject"].as_str(),
        Some("a raw message under test"),
        "case {case:?}: the common headers must carry the document's own subject, got {}",
        events[0]["mail"]
    );
}

/// One row of the destination table: how the destination is configured, which recipient the
/// send addresses, and the event types the subscribed queue must then receive.
struct DestinationCase {
    case: &'static str,
    enabled: bool,
    matching: &'static [&'static str],
    recipient: &'static str,
    received: &'static [&'static str],
}

/// The rows: the same send, differing in what the destination takes and what the recipient
/// does with the message.
const DESTINATION_CASES: &[DestinationCase] = &[
    DestinationCase {
        case: "a destination that is off",
        enabled: false,
        matching: &["SEND", "BOUNCE"],
        recipient: "bounce@simulator.amazonses.com",
        received: &[],
    },
    DestinationCase {
        case: "a destination matching only DELIVERY, for a recipient that took the message",
        enabled: true,
        matching: &["DELIVERY"],
        recipient: "success@simulator.amazonses.com",
        received: &["Delivery"],
    },
    DestinationCase {
        case: "a destination matching only DELIVERY, for a recipient that bounced",
        enabled: true,
        matching: &["DELIVERY"],
        recipient: "bounce@simulator.amazonses.com",
        received: &[],
    },
    DestinationCase {
        case: "a destination matching only COMPLAINT, for the complaining mailbox",
        enabled: true,
        matching: &["COMPLAINT"],
        recipient: "complaint@simulator.amazonses.com",
        received: &["Complaint"],
    },
    DestinationCase {
        case: "a destination matching every modelled event type",
        enabled: true,
        matching: EVERY_EVENT_TYPE,
        recipient: "success@simulator.amazonses.com",
        received: &["Send", "Delivery"],
    },
];

#[tokio::test]
async fn a_destination_receives_exactly_the_events_it_is_on_for_and_matches() {
    for row in DESTINATION_CASES {
        let case = row.case;
        let (router, engine) = router();
        let (topic, queue) =
            given_topic_with_subscriber(&router, case, REGION, "ses-send-events", Grant::Unscoped)
                .await;
        given_verified_sender(&router, &engine, case).await;
        given_configuration_set(&router, case, &topic, row.enabled, row.matching).await;

        let message_id = send(&router, case, row.recipient).await;
        // At least one poll, whatever the row expects: a row that expects NOTHING has to
        // read the queue to find it empty, and `drain_until` stops as soon as what it is
        // waiting for has arrived.
        let mut events =
            drain_until(&router, case, REGION, &queue, row.received.len().max(1)).await;
        // And one more read after it, so a row that expects one event would also see a
        // second one it must not have received.
        events.extend(drain(&router, case, REGION, &queue).await);
        let mut arrived = event_types(&events);
        arrived.sort();
        let mut wanted: Vec<String> = row.received.iter().map(|at| at.to_string()).collect();
        wanted.sort();
        assert_eq!(
            arrived, wanted,
            "case {case:?}: expected exactly {wanted:?} on the queue, got {events:?}"
        );
        for event in &events {
            assert_eq!(
                event["mail"]["messageId"].as_str(),
                Some(message_id.as_str()),
                "case {case:?}: every event must name the message it is about, got {event}"
            );
        }
    }
}
