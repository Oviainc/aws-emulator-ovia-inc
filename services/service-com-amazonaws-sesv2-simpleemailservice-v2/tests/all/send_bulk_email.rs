// SPDX-License-Identifier: CC0-1.0

//! `SendBulkEmail`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this service
//! registered — so routing, deserialization, the handler and serialization are all
//! exercised, and every accepted message is read back through `GetMessageInsights`.
//!
//! The rules these cases pin:
//!
//! * A bulk send composes ONE message per `BulkEmailEntries` entry from the request's one
//!   `DefaultContent` template, and what is the entry's own — its destination, the values
//!   substituted into the template, its headers, its tags — belongs to that message alone,
//!   while the request's `Default…` members apply to every entry that states nothing of
//!   its own. Each accepted entry answers a `MessageId` of its own.
//! * `Template$TemplateData` is "a set of key-value pairs. Each key defines a message
//!   variable in the template", so each `{{ name }}` the template writes is replaced by the
//!   value the data gives that name and a name the data does not give renders as nothing.
//! * `BulkEmailEntry$ReplacementHeaders`: a header the template defines and the entry does
//!   not is added as the template wrote it, a header the entry defines "takes precedence
//!   over any header of the same name specified in Template", and one the template does
//!   not define "will simply be added to the email as specified in BulkEmailEntry".
//! * A failure of the ENTRY is reported as that entry's `BulkEmailStatus`, with no
//!   `MessageId`, and the other entries are still sent; a failure of the REQUEST — its
//!   template, its sender, its configuration set, the account's switches — refuses the
//!   whole call and sends nothing.
//!
//! Where the published email event carries what these cases assert on, the cross-service
//! suite `core/router/tests/all/ses_bulk_send_entry_headers_override_the_template.rs` pins
//! it over the whole mounted service set: a message's headers reach a subscribed queue
//! only with SESv2, SNS and SQS all mounted.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, SentMessage,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const HOME: &str = "us-east-1";

/// The sender every accepted send is from, and the domain identity that authorises it.
const SENDER: &str = "sender@senders.example.com";
const SENDER_DOMAIN: &str = "senders.example.com";

/// The stored template the cases send through, and its parts.
const TEMPLATE: &str = "receipt";
const TEMPLATE_SUBJECT: &str = "Your receipt, {{name}}";
const TEMPLATE_TEXT: &str = "Thanks {{name}}, you paid {{amount}}.";

/// The account's 24-hour sending quota in the SES sandbox, which `GetAccount` reports as
/// `SendQuota$Max24HourSend` and every case that charges against it reads back.
const QUOTA: f64 = 200.0;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
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

    /// The `(Status, MessageId, Error)` of every `BulkEmailEntryResults` entry, in the
    /// order the response reported them. An absent member reads as the empty string.
    fn results(&self) -> Vec<(String, String, String)> {
        self.body
            .get("BulkEmailEntryResults")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        let member = |name: &str| {
                            row.get(name)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        (member("Status"), member("MessageId"), member("Error"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router, plus the shared engine, so a case can settle
/// the verification transition the clock owes an identity and read the parts of an
/// accepted message no implemented operation reports.
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

    /// One call, signed for `HOME`: the credential scope is what the handler reads the
    /// Region of the call from.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{HOME}.amazonaws.com{path}"))
            .header("host", format!("email.{HOME}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, \
                     SignedHeaders=host, Signature=00"
                ),
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

    /// `SendBulkEmail` with `body` as the whole request.
    async fn send_bulk(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-bulk-emails", Some(body))
            .await
    }

    /// `GetMessageInsights` for one accepted message.
    async fn insights(&self, message_id: &str) -> Reply {
        self.call("GET", &format!("/v2/email/insights/{message_id}"), None)
            .await
    }

    /// `GetAccount`, which reports how much of the sending quota the window has charged.
    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    /// `CreateEmailTemplate` with `content` as its `TemplateContent`.
    async fn create_template(&self, name: &str, content: Value) -> Reply {
        self.call(
            "POST",
            "/v2/email/templates",
            Some(json!({"TemplateName": name, "TemplateContent": content})),
        )
        .await
    }

    /// `CreateEmailIdentity`.
    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    /// `CreateConfigurationSet`.
    async fn create_configuration_set(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": name})),
        )
        .await
    }

    /// `PutConfigurationSetSendingOptions`.
    async fn set_configuration_set_sending(&self, name: &str, enabled: bool) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{name}/sending"),
            Some(json!({"SendingEnabled": enabled})),
        )
        .await
    }

    /// `PutAccountSendingAttributes`.
    async fn set_account_sending(&self, enabled: bool) -> Reply {
        self.call(
            "PUT",
            "/v2/email/account/sending",
            Some(json!({"SendingEnabled": enabled})),
        )
        .await
    }

    /// Bring every identity's verification transition into the past, so a case that sends
    /// from an identity the account holds does not race the span the clock owes it.
    fn settle_identities(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .expect("an instant before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
        store.settle(SystemTime::now());
    }

    /// The accepted message `message_id` names, as the outbound log holds it.
    ///
    /// Read from the store rather than over the wire for the parts of a message no
    /// implemented operation reports — its body parts and the headers added to it, which
    /// reach a client only as a published email event (the cross-service suite named in
    /// the module header).
    fn logged(&self, message_id: &str) -> SentMessage {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .sent_messages
            .iter()
            .find(|message| message.message_id == message_id)
            .cloned()
            .unwrap_or_else(|| panic!("the log must hold the accepted message {message_id}"))
    }

    /// Charge `recipients` against the account's quota by putting one already-accepted
    /// message in the log, the way an earlier send would have left it.
    fn charge_quota(&self, recipients: usize) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let now = SystemTime::now();
        store.sent_messages.push(SentMessage {
            message_id: "0100already-sent".to_string(),
            from: Some(SENDER.to_string()),
            to: (0..recipients)
                .map(|index| format!("charged-{index}@example.org"))
                .collect(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: None,
            body: None,
            body_html: None,
            configuration_set_name: None,
            reply_to: Vec::new(),
            feedback_forwarding: None,
            email_tags: Vec::new(),
            headers: Vec::new(),
            sent_at: now,
        });
        store.settle(now);
    }
}

/// A service holding the verified sending domain and the stored template every accepted
/// send below uses.
async fn seeded() -> Ses {
    let ses = Ses::new();
    let identity = ses.create_identity(SENDER_DOMAIN).await;
    assert_eq!(
        identity.status, 200,
        "expected the sending domain to be created, got {} {:?}",
        identity.status, identity.body
    );
    let template = ses
        .create_template(
            TEMPLATE,
            json!({"Subject": TEMPLATE_SUBJECT, "Text": TEMPLATE_TEXT}),
        )
        .await;
    assert_eq!(
        template.status, 200,
        "expected the template to be created, got {} {:?}",
        template.status, template.body
    );
    ses.settle_identities();
    ses
}

/// One `BulkEmailEntries` entry addressing `to`, with nothing of its own.
fn entry(to: &str) -> Value {
    json!({"Destination": {"ToAddresses": [to]}})
}

/// The request body of a bulk send through the stored template, with `data` as the default
/// template data and `entries` as its entries.
fn request(data: Option<&str>, entries: Vec<Value>) -> Value {
    let mut template = json!({"TemplateName": TEMPLATE});
    if let Some(data) = data {
        template["TemplateData"] = json!(data);
    }
    json!({
        "FromEmailAddress": SENDER,
        "DefaultContent": {"Template": template},
        "BulkEmailEntries": entries,
    })
}

#[tokio::test]
async fn the_default_template_data_has_to_be_a_set_of_key_value_pairs() {
    // The member is what makes a bulk send's content complete, so the rows span the ways a
    // request can fail to carry one: the member absent altogether (the recorded case), and
    // every JSON value that is no object — a list, each kind of scalar, and text that is no
    // JSON at all. The accepted rows below are the counter-case.
    let refused: &[(&str, Option<&str>, &str)] = &[
        (
            "no TemplateData member at all",
            None,
            "The default template data does not exist",
        ),
        (
            "a JSON list",
            Some("[]"),
            "The default template data is not a valid JSON object",
        ),
        (
            "a JSON string",
            Some("\"name\""),
            "The default template data is not a valid JSON object",
        ),
        (
            "a JSON number",
            Some("42"),
            "The default template data is not a valid JSON object",
        ),
        (
            "a JSON boolean",
            Some("true"),
            "The default template data is not a valid JSON object",
        ),
        (
            "JSON null",
            Some("null"),
            "The default template data is not a valid JSON object",
        ),
        (
            "text that is no JSON at all",
            Some("name=reader"),
            "The default template data is not a valid JSON object",
        ),
        (
            "an unterminated JSON object",
            Some("{\"name\":"),
            "The default template data is not a valid JSON object",
        ),
    ];

    for (case, data, expected) in refused {
        let ses = seeded().await;
        let reply = ses
            .send_bulk(request(
                *data,
                vec![entry("first@example.net"), entry("second@example.net")],
            ))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            *expected,
            "case {case:?}: expected the refusal to name the member, got {:?}",
            reply.message()
        );
        // A refused request sends nothing at all: no entry of it is charged against the
        // account's quota.
        let account = ses.account().await;
        assert_eq!(
            account.body.pointer("/SendQuota/SentLast24Hours"),
            Some(&json!(0.0)),
            "case {case:?}: a refused bulk send must charge nothing, got {:?}",
            account.body.pointer("/SendQuota")
        );
    }

    // The values that ARE a set of key-value pairs, including the two empty ones a caller
    // may send (`EmailTemplateData` is `length: min 0`).
    for (case, data) in [
        ("the empty string", ""),
        ("an empty JSON object", "{}"),
        ("one pair", "{\"name\":\"Ada\"}"),
        (
            "a pair the template names and one it does not",
            "{\"name\":\"Ada\",\"unused\":1}",
        ),
    ] {
        let ses = seeded().await;
        let reply = ses
            .send_bulk(request(Some(data), vec![entry("reader@example.net")]))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the send to be accepted, got {} {:?}",
            reply.status, reply.body
        );
        let results = reply.results();
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: expected one result, got {results:?}"
        );
        assert_eq!(
            results[0].0, "SUCCESS",
            "case {case:?}: expected the entry to be accepted, got {results:?}"
        );
    }
}

/// What a request-level gate case sets up before it sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// The default content names no template at all.
    NoTemplate,
    /// The template the request names is one the account does not hold.
    TemplateMissing,
    /// The request names a configuration set nothing created.
    ConfigurationSetMissing,
    /// The request names a configuration set whose own sending switch is off.
    ConfigurationSetSendingOff,
    /// The account's sending switch is off.
    AccountSendingOff,
    /// The sender is an address no identity authorises.
    UnverifiedSender,
    /// The request addresses one destination more than a bulk send may.
    TooManyDestinations,
}

/// Destinations one bulk send may address, which the handler refuses past.
const MAX_DESTINATIONS: usize = 50;

#[tokio::test]
async fn a_failure_of_the_request_refuses_the_whole_send() {
    let cases: &[(Gate, u16, &str, String)] = &[
        (
            Gate::NoTemplate,
            400,
            "BadRequestException",
            "The default template does not exist".to_string(),
        ),
        (
            Gate::TemplateMissing,
            404,
            "NotFoundException",
            "Template no-such-template does not exist.".to_string(),
        ),
        (
            Gate::ConfigurationSetMissing,
            404,
            "NotFoundException",
            "Configuration set <no-such-set> does not exist.".to_string(),
        ),
        (
            Gate::ConfigurationSetSendingOff,
            400,
            "SendingPausedException",
            "Sending is paused for configuration set paused-set".to_string(),
        ),
        (
            Gate::AccountSendingOff,
            400,
            "SendingPausedException",
            "The message can't be sent because the account's ability to send email is \
             currently paused."
                .to_string(),
        ),
        (
            Gate::UnverifiedSender,
            400,
            "MessageRejected",
            format!(
                "Email address is not verified. The following identities failed the check in \
                 region {}: stranger@nowhere.example",
                HOME.to_uppercase()
            ),
        ),
        (
            Gate::TooManyDestinations,
            400,
            "LimitExceededException",
            format!(
                "A bulk send may address no more than {MAX_DESTINATIONS} destinations, but {} \
                 were provided.",
                MAX_DESTINATIONS + 1
            ),
        ),
    ];

    for (gate, status, code, message) in cases {
        let ses = seeded().await;
        let mut body = request(Some("{\"name\":\"Ada\"}"), vec![entry("reader@example.net")]);
        match gate {
            Gate::NoTemplate => {
                body["DefaultContent"] = json!({});
            }
            Gate::TemplateMissing => {
                body["DefaultContent"]["Template"]["TemplateName"] = json!("no-such-template");
            }
            Gate::ConfigurationSetMissing => {
                body["ConfigurationSetName"] = json!("no-such-set");
            }
            Gate::ConfigurationSetSendingOff => {
                let created = ses.create_configuration_set("paused-set").await;
                assert_eq!(
                    created.status, 200,
                    "case {gate:?}: expected the set to be created, got {} {:?}",
                    created.status, created.body
                );
                let paused = ses.set_configuration_set_sending("paused-set", false).await;
                assert_eq!(
                    paused.status, 200,
                    "case {gate:?}: expected the set's sending to be switched off, got {} {:?}",
                    paused.status, paused.body
                );
                body["ConfigurationSetName"] = json!("paused-set");
            }
            Gate::AccountSendingOff => {
                let paused = ses.set_account_sending(false).await;
                assert_eq!(
                    paused.status, 200,
                    "case {gate:?}: expected the account's sending to be switched off, got {} {:?}",
                    paused.status, paused.body
                );
            }
            Gate::UnverifiedSender => {
                body["FromEmailAddress"] = json!("stranger@nowhere.example");
            }
            Gate::TooManyDestinations => {
                body["BulkEmailEntries"] = Value::Array(
                    (0..=MAX_DESTINATIONS)
                        .map(|index| entry(&format!("reader-{index}@example.net")))
                        .collect(),
                );
            }
        }

        let reply = ses.send_bulk(body).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (*status, *code),
            "case {gate:?}: expected {status} {code}, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            &reply.message(),
            message,
            "case {gate:?}: expected the refusal to name what stopped the send, got {:?}",
            reply.message()
        );
        let account = ses.account().await;
        assert_eq!(
            account.body.pointer("/SendQuota/SentLast24Hours"),
            Some(&json!(0.0)),
            "case {gate:?}: a refused bulk send must charge nothing, got {:?}",
            account.body.pointer("/SendQuota")
        );
    }

    // The bound itself is accepted: one destination fewer than the refusal above is a send
    // of exactly that many messages.
    let ses = seeded().await;
    let reply = ses
        .send_bulk(request(
            Some("{\"name\":\"Ada\"}"),
            (0..MAX_DESTINATIONS)
                .map(|index| entry(&format!("reader-{index}@example.net")))
                .collect(),
        ))
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"the destination bound itself\": expected the send to be accepted, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.results().len(),
        MAX_DESTINATIONS,
        "case \"the destination bound itself\": expected one result per destination, got {:?}",
        reply.results()
    );
}

/// One row of the per-entry table: the entry to send and what its own message must then
/// carry.
struct EntryCase {
    case: &'static str,
    /// The entry, as the request carries it.
    entry: fn() -> Value,
    /// The subject the template renders to for this entry.
    subject: &'static str,
    /// The recipients the message is addressed to, in `To`, `Cc`, `Bcc` order.
    recipients: &'static [&'static str],
    /// The `EmailTags` the accepted message carries.
    tags: &'static [(&'static str, &'static str)],
}

/// The rows: the entries differ along everything an entry states for itself — how its
/// destination is shaped, whether it states its own values for the template's variables,
/// and whether it states its own tags.
const ENTRY_CASES: &[EntryCase] = &[
    EntryCase {
        case: "an entry with nothing of its own",
        entry: || json!({"Destination": {"ToAddresses": ["plain@example.net"]}}),
        subject: "Your receipt, Ada",
        recipients: &["plain@example.net"],
        tags: &[("campaign", "spring")],
    },
    EntryCase {
        case: "an entry with its own values for the template's variables",
        entry: || {
            json!({
                "Destination": {"ToAddresses": ["own-data@example.net"]},
                "ReplacementEmailContent": {"ReplacementTemplate": {
                    "ReplacementTemplateData": "{\"name\":\"Grace\",\"amount\":\"$9\"}",
                }},
            })
        },
        subject: "Your receipt, Grace",
        recipients: &["own-data@example.net"],
        tags: &[("campaign", "spring")],
    },
    EntryCase {
        case: "an entry whose own values name no variable the template writes",
        entry: || {
            json!({
                "Destination": {"ToAddresses": ["other-data@example.net"]},
                "ReplacementEmailContent": {"ReplacementTemplate": {
                    "ReplacementTemplateData": "{\"unrelated\":\"value\"}",
                }},
            })
        },
        // The entry's own values REPLACE the request's default ones, so a variable neither
        // of them names renders as nothing.
        subject: "Your receipt, ",
        recipients: &["other-data@example.net"],
        tags: &[("campaign", "spring")],
    },
    EntryCase {
        case: "an entry addressing To, Cc and Bcc",
        entry: || {
            json!({
                "Destination": {
                    "ToAddresses": ["to-1@example.net", "to-2@example.net"],
                    "CcAddresses": ["cc@example.net"],
                    "BccAddresses": ["bcc@example.net"],
                },
            })
        },
        subject: "Your receipt, Ada",
        recipients: &[
            "to-1@example.net",
            "to-2@example.net",
            "cc@example.net",
            "bcc@example.net",
        ],
        tags: &[("campaign", "spring")],
    },
    EntryCase {
        case: "an entry addressing Cc alone",
        entry: || json!({"Destination": {"CcAddresses": ["cc-only@example.net"]}}),
        subject: "Your receipt, Ada",
        recipients: &["cc-only@example.net"],
        tags: &[("campaign", "spring")],
    },
    EntryCase {
        case: "an entry with its own tags",
        entry: || {
            json!({
                "Destination": {"ToAddresses": ["own-tags@example.net"]},
                "ReplacementTags": [{"Name": "campaign", "Value": "autumn"},
                                    {"Name": "segment", "Value": "new"}],
            })
        },
        subject: "Your receipt, Ada",
        recipients: &["own-tags@example.net"],
        tags: &[("campaign", "autumn"), ("segment", "new")],
    },
    EntryCase {
        case: "an entry with an empty tag list of its own",
        entry: || {
            json!({
                "Destination": {"ToAddresses": ["no-tags@example.net"]},
                "ReplacementTags": [],
            })
        },
        subject: "Your receipt, Ada",
        recipients: &["no-tags@example.net"],
        tags: &[],
    },
];

#[tokio::test]
async fn every_entry_gets_a_message_of_its_own_composed_from_the_one_template() {
    let ses = seeded().await;
    let mut body = request(
        Some("{\"name\":\"Ada\",\"amount\":\"$5\"}"),
        ENTRY_CASES.iter().map(|row| (row.entry)()).collect(),
    );
    body["DefaultEmailTags"] = json!([{"Name": "campaign", "Value": "spring"}]);

    let reply = ses.send_bulk(body).await;
    assert_eq!(
        reply.status, 200,
        "expected the bulk send to be accepted, got {} {:?}",
        reply.status, reply.body
    );
    let results = reply.results();
    assert_eq!(
        results.len(),
        ENTRY_CASES.len(),
        "expected one result per entry, in the order the entries were sent, got {results:?}"
    );

    let mut seen: Vec<String> = Vec::new();
    for (row, (status, message_id, error)) in ENTRY_CASES.iter().zip(&results) {
        let case = row.case;
        assert_eq!(
            (status.as_str(), error.as_str()),
            ("SUCCESS", ""),
            "case {case:?}: expected an accepted entry, got {status:?} {error:?}"
        );
        assert!(
            !message_id.is_empty(),
            "case {case:?}: an accepted entry must answer a MessageId of its own"
        );
        assert!(
            !seen.contains(message_id),
            "case {case:?}: every accepted message gets a unique MessageId, got {message_id:?} \
             among {seen:?}"
        );
        seen.push(message_id.clone());

        let read = ses.insights(message_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the accepted message to be readable, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("/Subject"),
            row.subject,
            "case {case:?}: expected the subject the template rendered for this entry, got {:?}",
            read.text("/Subject")
        );
        assert_eq!(
            read.text("/FromEmailAddress"),
            SENDER,
            "case {case:?}: every message of a bulk send is from the request's sender, got {:?}",
            read.text("/FromEmailAddress")
        );
        let addressed: Vec<String> = read
            .body
            .get("Insights")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.get("Destination")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            addressed,
            row.recipients
                .iter()
                .map(|address| (*address).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected this entry's own recipients, got {addressed:?}"
        );
        let tags: Vec<(String, String)> = read
            .body
            .get("EmailTags")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        let member = |name: &str| {
                            row.get(name)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        (member("Name"), member("Value"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            tags,
            row.tags
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<Vec<(String, String)>>(),
            "case {case:?}: expected the tags this entry's message carries, got {tags:?}"
        );
        // The text part is rendered from the same template with the same values, and no
        // implemented operation reports a message's body.
        let logged = ses.logged(message_id);
        let expected_text = row.subject.replace("Your receipt, ", "");
        assert!(
            logged
                .body
                .as_deref()
                .is_some_and(|text| text.starts_with(&format!("Thanks {expected_text},"))),
            "case {case:?}: expected the text part rendered with this entry's values, got {:?}",
            logged.body
        );
    }

    // Every recipient of every accepted message is charged against the account's quota.
    let charged: f64 = ENTRY_CASES
        .iter()
        .map(|row| row.recipients.len() as f64)
        .sum();
    let account = ses.account().await;
    assert_eq!(
        account.body.pointer("/SendQuota/SentLast24Hours"),
        Some(&json!(charged)),
        "expected every recipient of every message to be charged, got {:?}",
        account.body.pointer("/SendQuota")
    );
}

#[tokio::test]
async fn an_entry_that_names_no_message_is_the_only_one_that_fails() {
    // The rows span the failures that belong to ONE entry: a destination addressing
    // nobody, in each of the shapes that addresses nobody, and values for the template
    // that are no set of key-value pairs.
    let refused: &[(&str, Value, &str, &str)] = &[
        (
            "a destination with no addresses at all",
            json!({"Destination": {}}),
            "INVALID_PARAMETER",
            "Destination must contain at least one of ToAddresses, CcAddresses or \
             BccAddresses.",
        ),
        (
            "a destination whose address lists are all empty",
            json!({"Destination": {"ToAddresses": [], "CcAddresses": [], "BccAddresses": []}}),
            "INVALID_PARAMETER",
            "Destination must contain at least one of ToAddresses, CcAddresses or \
             BccAddresses.",
        ),
        (
            "replacement data that is a JSON list",
            json!({
                "Destination": {"ToAddresses": ["listy@example.net"]},
                "ReplacementEmailContent": {"ReplacementTemplate": {
                    "ReplacementTemplateData": "[1,2]",
                }},
            }),
            "INVALID_PARAMETER",
            "The replacement template data is not a valid JSON object",
        ),
        (
            "replacement data that is no JSON at all",
            json!({
                "Destination": {"ToAddresses": ["texty@example.net"]},
                "ReplacementEmailContent": {"ReplacementTemplate": {
                    "ReplacementTemplateData": "name=Ada",
                }},
            }),
            "INVALID_PARAMETER",
            "The replacement template data is not a valid JSON object",
        ),
    ];

    for (case, bad, status, error) in refused {
        let ses = seeded().await;
        let reply = ses
            .send_bulk(request(
                Some("{\"name\":\"Ada\"}"),
                vec![
                    entry("before@example.net"),
                    bad.clone(),
                    entry("after@example.net"),
                ],
            ))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: an entry's own failure is reported in the results, not as the \
             call's error, got {} {:?}",
            reply.status, reply.body
        );
        let results = reply.results();
        assert_eq!(
            results.len(),
            3,
            "case {case:?}: expected one result per entry, got {results:?}"
        );
        assert_eq!(
            (results[1].0.as_str(), results[1].2.as_str()),
            (*status, *error),
            "case {case:?}: expected the refused entry to carry what stopped it, got {:?}",
            results[1]
        );
        assert!(
            results[1].1.is_empty(),
            "case {case:?}: a refused entry carries no MessageId, got {:?}",
            results[1].1
        );
        for (index, expected_recipient) in [(0, "before@example.net"), (2, "after@example.net")] {
            assert_eq!(
                results[index].0, "SUCCESS",
                "case {case:?}: the entries beside the refused one are still sent, got {results:?}"
            );
            let read = ses.insights(&results[index].1).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected entry {index}'s message to be readable, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body.pointer("/Insights/0/Destination"),
                Some(&json!(expected_recipient)),
                "case {case:?}: expected entry {index}'s own recipient, got {:?}",
                read.body.get("Insights")
            );
        }
        // Only the accepted entries were charged.
        let account = ses.account().await;
        assert_eq!(
            account.body.pointer("/SendQuota/SentLast24Hours"),
            Some(&json!(2.0)),
            "case {case:?}: a refused entry must charge nothing, got {:?}",
            account.body.pointer("/SendQuota")
        );
    }
}

#[tokio::test]
async fn the_values_a_send_states_are_substituted_into_the_template() {
    // (case, the template's subject line, the values stated for it, the subject the send
    // must render). The rows span what a reference can be and what a value can be: a name
    // the data states and one it does not, the same name twice, whitespace inside the
    // braces, the unescaped spellings, a dotted path into the data, and each JSON scalar.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("one name the data states", "Hi {{name}}", "{\"name\":\"Ada\"}", "Hi Ada"),
        ("a name the data does not state", "Hi {{name}}", "{}", "Hi "),
        (
            "the same name twice",
            "{{name}} and {{name}}",
            "{\"name\":\"Ada\"}",
            "Ada and Ada",
        ),
        (
            "whitespace inside the braces",
            "Hi {{ name }}",
            "{\"name\":\"Ada\"}",
            "Hi Ada",
        ),
        (
            "the unescaped triple-brace spelling",
            "Hi {{{name}}}",
            "{\"name\":\"Ada & Grace\"}",
            "Hi Ada & Grace",
        ),
        (
            "the unescaped ampersand spelling",
            "Hi {{&name}}",
            "{\"name\":\"Ada\"}",
            "Hi Ada",
        ),
        (
            "a dotted path into the data",
            "Hi {{customer.first}} {{customer.last}}",
            "{\"customer\":{\"first\":\"Ada\",\"last\":\"Lovelace\"}}",
            "Hi Ada Lovelace",
        ),
        (
            "a dotted path the data stops short of",
            "Hi {{customer.first}}",
            "{\"customer\":{}}",
            "Hi ",
        ),
        (
            "a number",
            "You owe {{amount}}",
            "{\"amount\":42}",
            "You owe 42",
        ),
        (
            "a fractional number",
            "You owe {{amount}}",
            "{\"amount\":4.5}",
            "You owe 4.5",
        ),
        (
            "a boolean",
            "Subscribed: {{subscribed}}",
            "{\"subscribed\":true}",
            "Subscribed: true",
        ),
        (
            "a null value, which states nothing",
            "Hi {{name}}",
            "{\"name\":null}",
            "Hi ",
        ),
        (
            "a value that itself reads like a reference, substituted once",
            "Hi {{name}}",
            "{\"name\":\"{{name}}\"}",
            "Hi {{name}}",
        ),
        (
            "text with no reference in it at all",
            "Your receipt",
            "{\"name\":\"Ada\"}",
            "Your receipt",
        ),
        // Truncated text is no reference at all, and is sent as the template wrote it.
        (
            "an unterminated reference",
            "Hi {{name",
            "{\"name\":\"Ada\"}",
            "Hi {{name",
        ),
        (
            "an unterminated reference after one that closes",
            "Hi {{name}}, {{amount",
            "{\"name\":\"Ada\"}",
            "Hi Ada, {{amount",
        ),
        (
            "a closing brace pair with no opening one",
            "Hi name}}",
            "{\"name\":\"Ada\"}",
            "Hi name}}",
        ),
    ];

    for (case, subject, data, expected) in cases {
        let ses = Ses::new();
        let identity = ses.create_identity(SENDER_DOMAIN).await;
        assert_eq!(
            identity.status, 200,
            "case {case:?}: expected the sending domain to be created, got {} {:?}",
            identity.status, identity.body
        );
        let created = ses
            .create_template(TEMPLATE, json!({"Subject": subject, "Text": "body"}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be created, got {} {:?}",
            created.status, created.body
        );
        ses.settle_identities();

        let reply = ses
            .send_bulk(request(Some(data), vec![entry("reader@example.net")]))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the send to be accepted, got {} {:?}",
            reply.status, reply.body
        );
        let results = reply.results();
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: expected one result, got {results:?}"
        );
        let read = ses.insights(&results[0].1).await;
        assert_eq!(
            read.text("/Subject"),
            *expected,
            "case {case:?}: expected the rendered subject, got {:?}",
            read.text("/Subject")
        );
    }
}

#[tokio::test]
async fn the_html_part_escapes_what_it_substitutes_and_the_text_part_does_not() {
    // The same value in the three parts: the subject line and the text body are not HTML,
    // so the value belongs in them as the caller wrote it, while the HTML body's escaped
    // spelling (`{{name}}`) cannot be made to open or close an element and its unescaped
    // spelling (`{{{name}}}`) is the value as written.
    let ses = Ses::new();
    let identity = ses.create_identity(SENDER_DOMAIN).await;
    assert_eq!(
        identity.status, 200,
        "expected the sending domain to be created, got {} {:?}",
        identity.status, identity.body
    );
    let created = ses
        .create_template(
            TEMPLATE,
            json!({
                "Subject": "Hi {{name}}",
                "Text": "Hi {{name}}",
                "Html": "<p>{{name}}</p><p>{{{name}}}</p>",
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected the template to be created, got {} {:?}",
        created.status, created.body
    );
    ses.settle_identities();

    let reply = ses
        .send_bulk(request(
            Some("{\"name\":\"<b>Ada</b> & Grace\"}"),
            vec![entry("reader@example.net")],
        ))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected the send to be accepted, got {} {:?}",
        reply.status, reply.body
    );
    let results = reply.results();
    let message_id = &results[0].1;
    let read = ses.insights(message_id).await;
    assert_eq!(
        read.text("/Subject"),
        "Hi <b>Ada</b> & Grace",
        "expected the subject to carry the value as written, got {:?}",
        read.text("/Subject")
    );
    let logged = ses.logged(message_id);
    assert_eq!(
        logged.body.as_deref(),
        Some("Hi <b>Ada</b> & Grace"),
        "expected the text part to carry the value as written, got {:?}",
        logged.body
    );
    assert_eq!(
        logged.body_html.as_deref(),
        Some("<p>&lt;b&gt;Ada&lt;/b&gt; &amp; Grace</p><p><b>Ada</b> & Grace</p>"),
        "expected the HTML part to escape the value it substitutes and the unescaped \
         spelling to carry it as written, got {:?}",
        logged.body_html
    );
}

#[tokio::test]
async fn a_template_that_asks_to_be_interpreted_is_answered_as_unwritten() {
    // Substituting message variables is all this emulator does with a template, so a part
    // that writes anything else is answered as unwritten rather than sent with a body
    // nothing rendered. The rows span the constructs a template can write.
    for (case, subject) in [
        ("a section", "{{#each items}}{{this}}{{/each}}"),
        ("an inverted section", "{{^missing}}none{{/missing}}"),
        ("a partial", "Hi {{> greeting}}"),
        ("a comment", "Hi{{! not sent }}"),
        ("an inline partial", "{{#*inline \"p\"}}x{{/inline}}"),
        ("a helper call", "{{formatDate when}}"),
        ("a subscript", "{{items.[0]}}"),
        ("an empty reference", "Hi {{}}"),
    ] {
        let ses = Ses::new();
        let identity = ses.create_identity(SENDER_DOMAIN).await;
        assert_eq!(
            identity.status, 200,
            "case {case:?}: expected the sending domain to be created, got {} {:?}",
            identity.status, identity.body
        );
        let created = ses
            .create_template(TEMPLATE, json!({"Subject": subject, "Text": "body"}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the template to be created, got {} {:?}",
            created.status, created.body
        );
        ses.settle_identities();

        let reply = ses
            .send_bulk(request(Some("{}"), vec![entry("reader@example.net")]))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (501, "NotImplemented"),
            "case {case:?}: expected the send to be answered as unwritten, got {} {:?}",
            reply.status,
            reply.body
        );
        let account = ses.account().await;
        assert_eq!(
            account.body.pointer("/SendQuota/SentLast24Hours"),
            Some(&json!(0.0)),
            "case {case:?}: a send nothing was composed for must charge nothing, got {:?}",
            account.body.pointer("/SendQuota")
        );
    }
}

#[tokio::test]
async fn a_part_that_would_render_past_the_ceiling_fails_that_entry_alone() {
    // The one way a template amplifies what it is given: it names a variable many times.
    // The ceiling the handler renders against is the size a stored template may itself
    // have (SES's documented 500 KB maximum), so a subject line naming a 100 KB value six
    // times renders past it, and the entry that asked for it is the only one that fails.
    const RENDER_CEILING: usize = 500 * 1024;
    let value = "x".repeat(100 * 1024);
    let ses = Ses::new();
    let identity = ses.create_identity(SENDER_DOMAIN).await;
    assert_eq!(
        identity.status, 200,
        "expected the sending domain to be created, got {} {:?}",
        identity.status, identity.body
    );
    let created = ses
        .create_template(
            TEMPLATE,
            json!({"Subject": "{{blob}}{{blob}}{{blob}}{{blob}}{{blob}}{{blob}}", "Text": "body"}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected the template to be created, got {} {:?}",
        created.status, created.body
    );
    ses.settle_identities();

    let reply = ses
        .send_bulk(json!({
            "FromEmailAddress": SENDER,
            "DefaultContent": {"Template": {
                "TemplateName": TEMPLATE,
                "TemplateData": json!({"blob": "small"}).to_string(),
            }},
            "BulkEmailEntries": [
                json!({
                    "Destination": {"ToAddresses": ["oversized@example.net"]},
                    "ReplacementEmailContent": {"ReplacementTemplate": {
                        "ReplacementTemplateData": json!({"blob": value}).to_string(),
                    }},
                }),
                entry("sized@example.net"),
            ],
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected the bulk send to be accepted, got {}",
        reply.status
    );
    let results = reply.results();
    assert_eq!(
        results.len(),
        2,
        "expected one result per entry, got {} results",
        results.len()
    );
    assert_eq!(
        (results[0].0.as_str(), results[0].2.as_str()),
        (
            "FAILED",
            format!(
                "The message rendered for this destination is larger than the \
                 {RENDER_CEILING} bytes this emulator renders."
            )
            .as_str()
        ),
        "expected the oversized entry to fail with the ceiling it reached, got {:?} {:?}",
        results[0].0,
        results[0].2
    );
    assert!(
        results[0].1.is_empty(),
        "an entry nothing was composed for carries no MessageId, got {:?}",
        results[0].1
    );
    assert_eq!(
        results[1].0, "SUCCESS",
        "expected the entry whose own values fit to be sent, got {:?}",
        results[1]
    );
    let read = ses.insights(&results[1].1).await;
    assert_eq!(
        read.text("/Subject"),
        "smallsmallsmallsmallsmallsmall",
        "expected the sized entry's subject to be rendered from the request's own values, \
         got {:?}",
        read.text("/Subject")
    );
}

/// One row of the header table: the headers the template defines, the headers the entry
/// defines, and the headers the message then carries.
struct HeaderCase {
    case: &'static str,
    template: &'static [(&'static str, &'static str)],
    entry: Option<&'static [(&'static str, &'static str)]>,
    expected: &'static [(&'static str, &'static str)],
}

/// The rows, from the rule `BulkEmailEntry$ReplacementHeaders` documents: a header only
/// the template defines is added as the template wrote it; a header the entry defines
/// replaces the template's value for that name; a header only the entry defines is added
/// as the entry wrote it.
const HEADER_CASES: &[HeaderCase] = &[
    HeaderCase {
        case: "a header only the template defines",
        template: &[("X-Campaign", "spring")],
        entry: None,
        expected: &[("X-Campaign", "spring")],
    },
    HeaderCase {
        case: "a header the entry defines too",
        template: &[("X-Campaign", "spring")],
        entry: Some(&[("X-Campaign", "autumn")]),
        expected: &[("X-Campaign", "autumn")],
    },
    HeaderCase {
        case: "a header the entry defines under another case",
        template: &[("X-Campaign", "spring")],
        entry: Some(&[("x-campaign", "autumn")]),
        expected: &[("x-campaign", "autumn")],
    },
    HeaderCase {
        case: "a header only the entry defines",
        template: &[],
        entry: Some(&[("X-Segment", "new")]),
        expected: &[("X-Segment", "new")],
    },
    HeaderCase {
        case: "one header replaced and one left as the template wrote it",
        template: &[("X-Campaign", "spring"), ("X-Segment", "all")],
        entry: Some(&[("X-Segment", "new")]),
        expected: &[("X-Campaign", "spring"), ("X-Segment", "new")],
    },
    HeaderCase {
        case: "a header replaced and one added beside it",
        template: &[("X-Campaign", "spring")],
        entry: Some(&[("X-Campaign", "autumn"), ("X-Segment", "new")]),
        expected: &[("X-Campaign", "autumn"), ("X-Segment", "new")],
    },
    HeaderCase {
        case: "an entry that defines no headers at all",
        template: &[("X-Campaign", "spring")],
        entry: Some(&[]),
        expected: &[("X-Campaign", "spring")],
    },
    HeaderCase {
        case: "neither defines a header",
        template: &[],
        entry: None,
        expected: &[],
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

#[tokio::test]
async fn an_entrys_headers_are_the_templates_with_its_own_applied_over_them() {
    for row in HEADER_CASES {
        let case = row.case;
        let ses = seeded().await;
        let mut body = request(Some("{\"name\":\"Ada\"}"), Vec::new());
        body["DefaultContent"]["Template"]["Headers"] = header_list(row.template);
        let mut sent_entry = entry("reader@example.net");
        if let Some(headers) = row.entry {
            sent_entry["ReplacementHeaders"] = header_list(headers);
        }
        // A second entry with nothing of its own, so each row also shows that one entry's
        // headers are not another's.
        body["BulkEmailEntries"] = json!([sent_entry, entry("other@example.net")]);

        let reply = ses.send_bulk(body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the send to be accepted, got {} {:?}",
            reply.status, reply.body
        );
        let results = reply.results();
        assert_eq!(
            results.len(),
            2,
            "case {case:?}: expected one result per entry, got {results:?}"
        );

        let expected: Vec<(String, String)> = row
            .expected
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            ses.logged(&results[0].1).headers,
            expected,
            "case {case:?}: expected the entry's headers to be the template's with its own \
             applied over them, got {:?}",
            ses.logged(&results[0].1).headers
        );
        let untouched: Vec<(String, String)> = row
            .template
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            ses.logged(&results[1].1).headers,
            untouched,
            "case {case:?}: an entry that states no headers carries the template's, got {:?}",
            ses.logged(&results[1].1).headers
        );
    }
}

#[tokio::test]
async fn the_quota_is_charged_as_the_entries_are_accepted() {
    // The window has already charged all but two recipients of the account's quota, so the
    // rows of one bulk send fall on both sides of what is left: the first entry fits, the
    // second fills it exactly, and the third has nothing left to be sent with.
    let ses = seeded().await;
    ses.charge_quota(QUOTA as usize - 2);

    let reply = ses
        .send_bulk(request(
            Some("{\"name\":\"Ada\"}"),
            vec![
                entry("first@example.net"),
                entry("second@example.net"),
                entry("third@example.net"),
                json!({"Destination": {"ToAddresses": ["a@example.net", "b@example.net"]}}),
            ],
        ))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected the bulk send to be accepted, got {} {:?}",
        reply.status, reply.body
    );
    let results = reply.results();
    let statuses: Vec<&str> = results.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(
        statuses,
        vec![
            "SUCCESS",
            "SUCCESS",
            "ACCOUNT_DAILY_QUOTA_EXCEEDED",
            "ACCOUNT_DAILY_QUOTA_EXCEEDED",
        ],
        "expected the entries the quota had room for to be sent and the rest to report the \
         quota, got {results:?}"
    );
    assert_eq!(
        results[2].2,
        format!(
            "Daily message quota exceeded: {QUOTA} of {QUOTA} recipients already sent in the \
             last 24 hours."
        ),
        "expected the refused entry to name the quota and what the window has charged, got \
         {:?}",
        results[2].2
    );
    assert!(
        results[2].1.is_empty() && results[3].1.is_empty(),
        "an entry the quota had no room for carries no MessageId, got {results:?}"
    );
    let account = ses.account().await;
    assert_eq!(
        account.body.pointer("/SendQuota/SentLast24Hours"),
        Some(&json!(QUOTA)),
        "expected the quota to be charged for exactly the accepted recipients, got {:?}",
        account.body.pointer("/SendQuota")
    );
}

#[tokio::test]
async fn a_template_the_request_carries_or_addresses_by_arn_composes_the_same_message() {
    // The three ways a send names its template: the stored template by name, the same
    // stored template by the ARN that addresses it, and the content carried in the request
    // itself.
    let ses = seeded().await;
    let account_id = ::engine::emulator_account_id();
    let cases: &[(&str, Value, &str)] = &[
        (
            "the stored template by name",
            json!({"TemplateName": TEMPLATE, "TemplateData": "{\"name\":\"Ada\"}"}),
            "Your receipt, Ada",
        ),
        (
            "the stored template by ARN",
            json!({
                "TemplateArn": format!("arn:aws:ses:{HOME}:{account_id}:template/{TEMPLATE}"),
                "TemplateData": "{\"name\":\"Ada\"}",
            }),
            "Your receipt, Ada",
        ),
        (
            "content the request carries itself",
            json!({
                "TemplateContent": {"Subject": "Inline {{name}}", "Text": "body"},
                "TemplateData": "{\"name\":\"Ada\"}",
            }),
            "Inline Ada",
        ),
    ];

    for (case, template, expected) in cases {
        let reply = ses
            .send_bulk(json!({
                "FromEmailAddress": SENDER,
                "DefaultContent": {"Template": template},
                "BulkEmailEntries": [entry("reader@example.net")],
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the send to be accepted, got {} {:?}",
            reply.status, reply.body
        );
        let results = reply.results();
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: expected one result, got {results:?}"
        );
        let read = ses.insights(&results[0].1).await;
        assert_eq!(
            read.text("/Subject"),
            *expected,
            "case {case:?}: expected the subject this template renders, got {:?}",
            read.text("/Subject")
        );
    }
}
