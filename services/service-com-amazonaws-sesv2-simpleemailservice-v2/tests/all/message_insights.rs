// SPDX-License-Identifier: CC0-1.0

//! `GetMessageInsights`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the handler and serialization are all
//! exercised, and what a send persisted is read back through the insights report.
//!
//! The rule these cases pin: the call reports the accepted message its `MessageId`
//! names, computed from the outbound-message log — the from address, subject and
//! tags exactly as they were accepted, and one insights entry per recipient in
//! `To`, `Cc`, `Bcc` order carrying the acceptance event — while a `MessageId`
//! outside the form SES issues for a message is refused with a 400
//! `BadRequestException` and a well-formed id the account holds no message for with
//! a 404 `NotFoundException`; and the report is a read, so it moves nothing a later
//! caller sees. The tables below span what the rule generalises over: the number of
//! recipients and which fields they are addressed in, the number of tags, a message
//! with and without tags, the ways a value can fall outside the id form, and the
//! shapes a well-formed-but-unheld id takes.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `EventType`'s value for the acceptance of a send, as the model spells it
/// (`smithy_query(query="shape",shape="EventType")` lists `SEND`, `REJECT`,
/// `BOUNCE`, `COMPLAINT`, `DELIVERY`, `OPEN`, `CLICK`, `RENDERING_FAILURE`,
/// `DELIVERY_DELAY` and `SUBSCRIPTION`). Accepting the message is the only one of
/// those this emulator performs.
const EVENT_TYPE_SEND: &str = "SEND";

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, absent reported as an empty string.
    fn string(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// read the log the report is computed from.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn send(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    async fn insights(&self, message_id: &str) -> Reply {
        self.call("GET", &format!("/v2/email/insights/{message_id}"), None)
            .await
    }

    /// The account report, so a case can show a read moved nothing about the
    /// account's sending either.
    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    /// The ids the outbound-message log holds, oldest first.
    fn logged_message_ids(&self) -> Vec<String> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .sent_messages
            .iter()
            .map(|message| message.message_id.clone())
            .collect()
    }

    /// Bring every identity's verification transition into the past, so a case that
    /// sends does not race the span the clock owes it.
    fn settle_identities(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant just before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
    }

    /// A service with one verified sending domain, ready to send.
    async fn with_verified_domain(domain: &str) -> Self {
        let ses = Self::new();
        assert_eq!(
            ses.create_identity(domain).await.status,
            200,
            "expected the identity create for {domain:?} to succeed"
        );
        ses.settle_identities();
        ses
    }
}

/// One accepted message, and what its insights report has to say about it.
struct SendCase {
    name: &'static str,
    from: &'static str,
    to: Vec<&'static str>,
    cc: Vec<&'static str>,
    bcc: Vec<&'static str>,
    subject: &'static str,
    /// `EmailTags`, in the order the request carries them.
    tags: Vec<(&'static str, &'static str)>,
}

impl SendCase {
    fn request(&self) -> Value {
        let mut destination = json!({ "ToAddresses": self.to });
        if !self.cc.is_empty() {
            destination["CcAddresses"] = json!(self.cc);
        }
        if !self.bcc.is_empty() {
            destination["BccAddresses"] = json!(self.bcc);
        }
        let mut body = json!({
            "FromEmailAddress": self.from,
            "Destination": destination,
            "Content": {"Simple": {
                "Subject": {"Data": self.subject},
                "Body": {"Text": {"Data": "body text"}},
            }},
        });
        if !self.tags.is_empty() {
            body["EmailTags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            );
        }
        body
    }

    /// Every recipient, in the `To`, `Cc`, `Bcc` order the insights are reported in.
    fn recipients(&self) -> Vec<&'static str> {
        self.to
            .iter()
            .chain(self.cc.iter())
            .chain(self.bcc.iter())
            .copied()
            .collect()
    }
}

/// The accepted messages the rule has to hold for: one recipient and several, the
/// recipients spread across the three destination fields, no tags, one tag and
/// several tags whose order the report keeps, and different senders in the one
/// verified domain.
fn send_cases() -> Vec<SendCase> {
    vec![
        SendCase {
            name: "one-recipient-no-tags",
            from: "sender@insights.example.com",
            to: vec!["one@example.org"],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "a single recipient",
            tags: Vec::new(),
        },
        SendCase {
            name: "two-recipients-one-tag",
            from: "notices@insights.example.com",
            to: vec!["first@example.org", "second@example.net"],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "two recipients",
            tags: vec![("campaign", "spring")],
        },
        SendCase {
            name: "to-and-cc-three-tags",
            from: "sender@insights.example.com",
            to: vec!["to@example.org"],
            cc: vec!["cc@example.net"],
            bcc: Vec::new(),
            subject: "copied in",
            tags: vec![("campaign", "summer"), ("tier", "gold"), ("locale", "en-GB")],
        },
        SendCase {
            name: "to-cc-and-bcc",
            from: "bulk@insights.example.com",
            to: vec!["to-a@example.org", "to-b@example.org"],
            cc: vec!["cc-a@example.net"],
            bcc: vec!["bcc-a@example.com", "bcc-b@example.com"],
            subject: "everyone at once",
            tags: vec![("campaign", "winter")],
        },
    ]
}

/// The epoch seconds a serialized `Timestamp` member carries.
fn timestamp_seconds(event: &Value, case: &str) -> f64 {
    event
        .get("Timestamp")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an event Timestamp, got {event:?}"))
}

/// Seconds since the epoch at `instant`, for comparing against a reported
/// timestamp.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

#[tokio::test]
async fn the_report_is_the_accepted_message_the_id_names() {
    let ses = Ses::with_verified_domain("insights.example.com").await;

    let mut sent: Vec<(SendCase, String, f64, f64)> = Vec::new();
    for case in send_cases() {
        let before = epoch_seconds(SystemTime::now());
        let accepted = ses.send(case.request()).await;
        let after = epoch_seconds(SystemTime::now());
        assert_eq!(
            accepted.status,
            200,
            "case {:?}: expected the send to be accepted, got {} {:?}",
            case.name,
            accepted.status,
            accepted.body
        );
        let message_id = accepted.string("MessageId");
        assert!(
            !message_id.is_empty(),
            "case {:?}: expected the send to return a MessageId, got {:?}",
            case.name,
            accepted.body
        );
        sent.push((case, message_id, before, after));
    }

    for (case, message_id, before, after) in &sent {
        let name = case.name;
        let reported = ses.insights(message_id).await;
        assert_eq!(
            reported.status, 200,
            "case {name:?}: expected 200 from the insights report, got {} {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            reported.string("MessageId"),
            *message_id,
            "case {name:?}: expected the reported MessageId to be the one the send returned, got {:?}",
            reported.body
        );
        assert_eq!(
            reported.string("FromEmailAddress"),
            case.from,
            "case {name:?}: expected FromEmailAddress {:?}, got {:?}",
            case.from,
            reported.body
        );
        assert_eq!(
            reported.string("Subject"),
            case.subject,
            "case {name:?}: expected Subject {:?}, got {:?}",
            case.subject,
            reported.body
        );

        let expected_tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(tag, value)| json!({ "Name": tag, "Value": value }))
            .collect();
        assert_eq!(
            reported.body.get("EmailTags"),
            Some(&Value::Array(expected_tags.clone())),
            "case {name:?}: expected EmailTags {:?}, got {:?}",
            Value::Array(expected_tags),
            reported.body
        );

        let insights = reported
            .body
            .get("Insights")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {name:?}: expected Insights, got {:?}", reported.body))
            .clone();
        let destinations: Vec<String> = insights
            .iter()
            .map(|entry| {
                entry
                    .get("Destination")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            destinations,
            case.recipients(),
            "case {name:?}: expected one insights entry per recipient in To, Cc, Bcc order, got {insights:?}"
        );

        for entry in &insights {
            let events = entry
                .get("Events")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {name:?}: expected Events, got {entry:?}"));
            assert_eq!(
                events.len(),
                1,
                "case {name:?}: expected the acceptance event alone, got {events:?}"
            );
            let event = &events[0];
            assert_eq!(
                event.get("Type").and_then(Value::as_str),
                Some(EVENT_TYPE_SEND),
                "case {name:?}: expected the event type {EVENT_TYPE_SEND:?}, got {event:?}"
            );
            // Nothing here delivers, opens or bounces a message, so no event
            // details have happened to report, and the recipient's ISP is not
            // observable.
            assert!(
                event.get("Details").is_none(),
                "case {name:?}: expected no event Details, got {event:?}"
            );
            assert!(
                entry.get("Isp").is_none(),
                "case {name:?}: expected no Isp, got {entry:?}"
            );
            let stamp = timestamp_seconds(event, name);
            assert!(
                stamp >= before - 1.0 && stamp <= after + 1.0,
                "case {name:?}: expected the acceptance timestamp within [{}, {}], got {stamp}",
                before - 1.0,
                after + 1.0
            );
        }
    }

    // One message's report is its own: no id answers with another's insights.
    for (case, message_id, _, _) in &sent {
        for (other, other_id, _, _) in &sent {
            if message_id == other_id {
                continue;
            }
            let reported = ses.insights(other_id).await;
            assert_ne!(
                reported.string("Subject"),
                case.subject,
                "case {:?}: expected {:?}'s report to be about its own message, got {:?}",
                case.name,
                other.name,
                reported.body
            );
        }
    }
}

#[tokio::test]
async fn a_message_id_outside_the_issued_form_is_a_bad_request() {
    let ses = Ses::with_verified_domain("malformed.example.com").await;
    let accepted = ses
        .send(json!({
            "FromEmailAddress": "sender@malformed.example.com",
            "Destination": { "ToAddresses": ["rcpt@example.org"] },
            "Content": {"Simple": {"Subject": {"Data": "held"}, "Body": {"Text": {"Data": "b"}}}},
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "expected the send to be accepted, got {} {:?}",
        accepted.status, accepted.body
    );
    let held = accepted.string("MessageId");

    // Each row falls outside the form a message id takes in a different way: no
    // hyphen-separated field at all, fields that are not hexadecimal, a hexadecimal
    // value without the version prefix, the prefix with nothing after it, an empty
    // field, and the id of a message the log does hold with its prefix changed.
    let rows: Vec<(&str, String)> = vec![
        ("no-fields", "garbage".to_string()),
        ("words", "not-a-message-id".to_string()),
        (
            "uuid-without-the-version-prefix",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("prefix-only", "0100".to_string()),
        ("one-field", "0100abcdef012345".to_string()),
        ("empty-field", "0100abcdef012345--000000".to_string()),
        ("non-hex-digit", "0100abcdefg12345-0000000000000001-000000".to_string()),
        (
            "a-held-id-with-another-version",
            held.replacen("0100", "0200", 1),
        ),
    ];

    let logged = ses.logged_message_ids();
    for (name, message_id) in &rows {
        let refused = ses.insights(message_id).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {name:?} ({message_id:?}): expected a 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.message(),
            "Invalid MessageId",
            "case {name:?} ({message_id:?}): expected the invalid-id message, got {:?}",
            refused.message()
        );
        assert_eq!(
            ses.logged_message_ids(),
            logged,
            "case {name:?}: expected the refusal to leave the message log alone, got {:?}",
            ses.logged_message_ids()
        );
    }

    // The message the account does hold still reports, after every refusal.
    let reported = ses.insights(&held).await;
    assert_eq!(
        (reported.status, reported.string("MessageId")),
        (200, held.clone()),
        "expected the held message to still report, got {} {:?}",
        reported.status,
        reported.body
    );
}

#[tokio::test]
async fn a_well_formed_id_the_account_holds_no_message_for_is_not_found() {
    let ses = Ses::with_verified_domain("unheld.example.com").await;
    let accepted = ses
        .send(json!({
            "FromEmailAddress": "sender@unheld.example.com",
            "Destination": { "ToAddresses": ["rcpt@example.org"] },
            "Content": {"Simple": {"Subject": {"Data": "held"}, "Body": {"Text": {"Data": "b"}}}},
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "expected the send to be accepted, got {} {:?}",
        accepted.status, accepted.body
    );
    let held = accepted.string("MessageId");

    // Each row is a value inside the id form that names no accepted message: the
    // three-field shape, the longer shape the service also issues, an id one digit
    // away from the held one, and an id of a different field count entirely.
    let neighbour = {
        let (head, last) = held.split_at(held.len() - 1);
        let moved = if last == "0" { "1" } else { "0" };
        format!("{head}{moved}")
    };
    let rows: Vec<(&str, String)> = vec![
        (
            "three-fields",
            "0100000000000000-0000000000000000-000000".to_string(),
        ),
        (
            "many-fields",
            "0100018f1d4b5e6a-1b2c3d4e-5f60-7182-93a4-b5c6d7e8f901-000000".to_string(),
        ),
        ("one-digit-from-a-held-id", neighbour),
        ("two-fields", "0100abcdef012345-000000".to_string()),
    ];

    let logged = ses.logged_message_ids();
    for (name, message_id) in &rows {
        assert!(
            !logged.contains(message_id),
            "case {name:?}: expected {message_id:?} to name no accepted message, got {logged:?}"
        );
        let refused = ses.insights(message_id).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (404, "NotFoundException"),
            "case {name:?} ({message_id:?}): expected a 404 NotFoundException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.message(),
            format!("Message {message_id} does not exist."),
            "case {name:?}: expected the not-found message naming {message_id:?}, got {:?}",
            refused.message()
        );
        assert_eq!(
            ses.logged_message_ids(),
            logged,
            "case {name:?}: expected the refusal to leave the message log alone, got {:?}",
            ses.logged_message_ids()
        );
    }

    // An id this account's log holds is not held by another account's: a service
    // that never accepted it answers not-found for the very same value.
    let elsewhere = Ses::new();
    let refused = elsewhere.insights(&held).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: expected a 404 NotFoundException from a service that accepted nothing, got {} {:?}",
        "another-account",
        refused.status,
        refused.body
    );
}

#[tokio::test]
async fn the_report_moves_nothing_a_later_caller_sees() {
    let ses = Ses::with_verified_domain("repeat.example.com").await;
    let accepted = ses
        .send(json!({
            "FromEmailAddress": "sender@repeat.example.com",
            "Destination": { "ToAddresses": ["one@example.org", "two@example.org"] },
            "Content": {"Simple": {"Subject": {"Data": "read twice"}, "Body": {"Text": {"Data": "b"}}}},
            "EmailTags": [{"Name": "campaign", "Value": "repeat"}],
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "expected the send to be accepted, got {} {:?}",
        accepted.status, accepted.body
    );
    let held = accepted.string("MessageId");

    let account_before = ses.account().await.body;
    let logged = ses.logged_message_ids();
    let first = ses.insights(&held).await;
    assert_eq!(
        first.status, 200,
        "case first: expected 200 from the insights report, got {} {:?}",
        first.status, first.body
    );
    for read in ["second", "third"] {
        let again = ses.insights(&held).await;
        assert_eq!(
            again.status, 200,
            "case {read:?}: expected 200 from the insights report, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.body, first.body,
            "case {read:?}: expected the same report as the first read, got {:?}",
            again.body
        );
    }
    assert_eq!(
        ses.logged_message_ids(),
        logged,
        "expected the reads to leave the message log alone, got {:?}",
        ses.logged_message_ids()
    );
    assert_eq!(
        ses.account().await.body,
        account_before,
        "expected the reads to leave the account report alone, got {:?}",
        ses.account().await.body
    );
}
