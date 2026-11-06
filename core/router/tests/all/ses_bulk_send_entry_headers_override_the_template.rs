// SPDX-License-Identifier: CC0-1.0

//! The headers one `SendBulkEmail` entry's message carries reach the queue subscribed to
//! the configuration set's SNS event destination — through the WHOLE mounted service set,
//! the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test is `BulkEmailEntry$ReplacementHeaders`' own: the headers of the
//! message one entry gets are the request template's "list of message headers that will be
//! added to the email message" with that entry's applied over them —
//!
//! * "if a header is specified in `Template` but not in `BulkEmailEntry`, the header from
//!   `Template` will be added to the outgoing email";
//! * a header specified in `BulkEmailEntry` "takes precedence over any header of the same
//!   name specified in `Template`", so "the value from `BulkEmailEntry` will replace the
//!   header's value in the email";
//! * "if the header is not defined within `Template`, it will simply be added to the email
//!   as specified in `BulkEmailEntry`".
//!
//! — and one entry's headers are its own message's alone, whatever its siblings in the same
//! call state.
//!
//! Three services have to agree for that to be observable — SESv2 publishes the event of
//! each accepted message, SNS fans it out, SQS holds what arrived — and the publish SESv2
//! makes is an in-process call addressed the way a client's own `sns`-scoped call is, so
//! WHICH mounted model claims it is only decidable with every service mounted. The
//! crate-local suite
//! `services/service-com-amazonaws-sesv2-simpleemailservice-v2/tests/all/send_bulk_email.rs`
//! pins the merge itself over a table of template/entry pairs; what only this suite can
//! reach is the published event that carries it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    drain, drain_until, given_topic_with_subscriber, Grant,
};

/// The Region the whole wiring of this case lives in.
const REGION: &str = "us-west-2";

/// The domain identity that authorises the send, and the sender it covers.
const SENDER_DOMAIN: &str = "senders.example.com";
const SENDER: &str = "sender@senders.example.com";

/// The configuration set the send names, the event destination hung off it, and the stored
/// template the send composes its messages from.
const SET: &str = "bulk-event-publication";
const DESTINATION: &str = "events";
const TEMPLATE: &str = "receipt";

/// The headers the request's template defines for every message of the send.
const TEMPLATE_HEADERS: &[(&str, &str)] = &[("X-Campaign", "spring"), ("X-Segment", "all")];

/// The whole emulator over one engine, exactly as the binary assembles it, with the engine
/// kept so the case can bring the verification transition the clock owes an identity into
/// the past instead of waiting out its span.
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

/// One row of the entry table: what the entry states for its own headers, and the headers
/// the event of its message must then report.
struct EntryCase {
    case: &'static str,
    /// The recipient the entry addresses, which is also how its event is told apart.
    recipient: &'static str,
    /// `BulkEmailEntry$ReplacementHeaders`, or `None` when the entry states none.
    replacement: Option<&'static [(&'static str, &'static str)]>,
    /// The headers added to this entry's message, beside the envelope headers SES
    /// assembles from the message's own members.
    added: &'static [(&'static str, &'static str)],
}

/// The rows: one entry per clause of the documented rule, in one call, so each row also
/// shows that one entry's headers are not another's.
const ENTRY_CASES: &[EntryCase] = &[
    EntryCase {
        case: "an entry that states no headers of its own",
        recipient: "default@example.net",
        replacement: None,
        added: TEMPLATE_HEADERS,
    },
    EntryCase {
        case: "an entry that replaces one of the template's headers",
        recipient: "replaced@example.net",
        replacement: Some(&[("X-Campaign", "autumn")]),
        added: &[("X-Campaign", "autumn"), ("X-Segment", "all")],
    },
    EntryCase {
        case: "an entry that replaces one header and adds one the template does not define",
        recipient: "added@example.net",
        replacement: Some(&[("X-Segment", "new"), ("X-Priority", "high")]),
        added: &[
            ("X-Campaign", "spring"),
            ("X-Segment", "new"),
            ("X-Priority", "high"),
        ],
    },
];

/// `headers` as a request carries a `MessageHeaderList`.
fn header_list(headers: &[(&str, &str)]) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(name, value)| json!({"Name": name, "Value": value}))
            .collect(),
    )
}

/// The `(name, value)` pairs a published event reports under `mail.headers`.
fn published_headers(event: &Value) -> Vec<(String, String)> {
    event["mail"]["headers"]
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
        .unwrap_or_default()
}

#[tokio::test]
async fn each_bulk_entrys_event_reports_the_templates_headers_with_its_own_applied_over_them() {
    let case = "a bulk send through a configuration set publishing its SEND events";
    let (router, engine) = router();
    let (topic, queue) =
        given_topic_with_subscriber(&router, case, REGION, "ses-bulk-send-events", Grant::Unscoped)
            .await;

    // A verified sending domain, so the send is authorised.
    sesv2_ok(
        &router,
        case,
        "POST",
        "/v2/email/identities",
        Some(json!({"EmailIdentity": SENDER_DOMAIN})),
    )
    .await;
    {
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

    // The template the send composes every message from, and the configuration set that
    // publishes the acceptance of each of them to the topic the queue is subscribed to.
    sesv2_ok(
        &router,
        case,
        "POST",
        "/v2/email/templates",
        Some(json!({
            "TemplateName": TEMPLATE,
            "TemplateContent": {"Subject": "Your receipt, {{name}}", "Text": "Thanks {{name}}."},
        })),
    )
    .await;
    sesv2_ok(
        &router,
        case,
        "POST",
        "/v2/email/configuration-sets",
        Some(json!({"ConfigurationSetName": SET})),
    )
    .await;
    sesv2_ok(
        &router,
        case,
        "POST",
        &format!("/v2/email/configuration-sets/{SET}/event-destinations"),
        Some(json!({
            "EventDestinationName": DESTINATION,
            "EventDestination": {
                "Enabled": true,
                "MatchingEventTypes": ["SEND"],
                "SnsDestination": {"TopicArn": topic},
            },
        })),
    )
    .await;

    let entries: Vec<Value> = ENTRY_CASES
        .iter()
        .map(|row| {
            let mut entry = json!({"Destination": {"ToAddresses": [row.recipient]}});
            if let Some(headers) = row.replacement {
                entry["ReplacementHeaders"] = header_list(headers);
            }
            entry
        })
        .collect();
    let answered = sesv2_ok(
        &router,
        case,
        "POST",
        "/v2/email/outbound-bulk-emails",
        Some(json!({
            "FromEmailAddress": SENDER,
            "ConfigurationSetName": SET,
            "DefaultContent": {"Template": {
                "TemplateName": TEMPLATE,
                "TemplateData": "{\"name\":\"Ada\"}",
                "Headers": header_list(TEMPLATE_HEADERS),
            }},
            "BulkEmailEntries": entries,
        })),
    )
    .await;

    let results = answered.body["BulkEmailEntryResults"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        results.len(),
        ENTRY_CASES.len(),
        "case {case:?}: expected one result per entry, got {}",
        answered.body
    );

    // One `Send` event per accepted message reaches the queue.
    let events = drain_until(&router, case, REGION, &queue, ENTRY_CASES.len()).await;
    assert_eq!(
        events.len(),
        ENTRY_CASES.len(),
        "case {case:?}: expected the acceptance of every message to be published, got {events:?}"
    );

    for (row, result) in ENTRY_CASES.iter().zip(&results) {
        let case = row.case;
        assert_eq!(
            result["Status"].as_str(),
            Some("SUCCESS"),
            "case {case:?}: expected the entry to be accepted, got {result}"
        );
        let message_id = result["MessageId"].as_str().unwrap_or_default();
        assert!(
            !message_id.is_empty(),
            "case {case:?}: an accepted entry must answer a MessageId of its own, got {result}"
        );
        let published: Vec<&Value> = events
            .iter()
            .filter(|event| event["mail"]["messageId"].as_str() == Some(message_id))
            .collect();
        assert_eq!(
            published.len(),
            1,
            "case {case:?}: expected exactly one event naming {message_id}, got {events:?}"
        );
        let event = published[0];
        assert_eq!(
            event["eventType"].as_str(),
            Some("Send"),
            "case {case:?}: the destination matches SEND alone, got {event}"
        );

        // The envelope headers SES assembles from the message's own members, and then the
        // headers the send added to it: the template's, with this entry's applied over
        // them.
        let mut expected: Vec<(String, String)> = vec![
            ("From".to_string(), SENDER.to_string()),
            ("To".to_string(), row.recipient.to_string()),
            ("Subject".to_string(), "Your receipt, Ada".to_string()),
        ];
        expected.extend(
            row.added
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
        );
        assert_eq!(
            published_headers(event),
            expected,
            "case {case:?}: expected the event to report this message's own headers, got {event}"
        );
    }

    assert!(
        drain(&router, case, REGION, &queue).await.is_empty(),
        "case {case:?}: nothing else may be published for this send"
    );
}
