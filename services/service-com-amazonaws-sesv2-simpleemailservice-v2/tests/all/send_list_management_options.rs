// SPDX-License-Identifier: CC0-1.0

//! `SendEmail`'s `ListManagementOptions`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reads the stored contact list back through `ListContacts`.
//!
//! The rule these cases pin: "an object used to specify a list or topic to which an
//! email belongs, which will be used when a contact chooses to unsubscribe" names a
//! contact list this account holds and a topic that list publishes, so a send resolves
//! both against the contact-list table before it checks anything about the message, and
//! a value either member resolves to no resource for is a terminal 404
//! `NotFoundException` — whichever of the two it was. An accepted send makes each of its
//! recipients a contact of that list, since the unsubscribe link list management stamps
//! into the message unsubscribes a contact of it, and a recipient the send puts on the
//! list is subscribed to the topic the message belongs to — the subscription that link
//! reverses — while a send that names no topic states no preference at all.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
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

    /// The contact addresses a `ListContacts` reply reported, in the order it
    /// reported them.
    fn contacts(&self) -> Vec<String> {
        self.body
            .get("Contacts")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.get("EmailAddress")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// settle the verification transition the clock owes an identity.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

/// The Region every case that does not name its own is signed for.
const HOME: &str = "us-east-1";

/// The contact list the cases send against, and the topic it publishes.
const LIST: &str = "newsletter";
const TOPIC: &str = "weekly";

/// The sender every accepted send is from, and the domain identity that authorises
/// it.
const SENDER: &str = "sender@senders.example.com";
const SENDER_DOMAIN: &str = "senders.example.com";

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// One call, signed for `region`: the credential scope is what the handler reads
    /// the Region of the call from.
    async fn call(&self, region: &str, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{region}.amazonaws.com{path}"))
            .header("host", format!("email.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, \
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

    /// `CreateContactList` with one topic.
    async fn create_list(&self, name: &str) -> Reply {
        self.create_list_with_topics(name, &[(TOPIC, "OPT_IN")]).await
    }

    /// `CreateContactList` publishing each `(TopicName, DefaultSubscriptionStatus)`
    /// supplied, in that order.
    async fn create_list_with_topics(&self, name: &str, topics: &[(&str, &str)]) -> Reply {
        let topics: Vec<Value> = topics
            .iter()
            .map(|(topic, default_status)| {
                json!({
                    "TopicName": topic,
                    "DisplayName": format!("{topic} news"),
                    "DefaultSubscriptionStatus": default_status,
                })
            })
            .collect();
        self.call(
            HOME,
            "POST",
            "/v2/email/contact-lists",
            Some(json!({ "ContactListName": name, "Topics": topics })),
        )
        .await
    }

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            HOME,
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn list_contacts(&self, list: &str) -> Reply {
        self.call(
            HOME,
            "POST",
            &format!("/v2/email/contact-lists/{list}/contacts/list"),
            Some(json!({})),
        )
        .await
    }

    async fn get_contact(&self, list: &str, address: &str) -> Reply {
        self.call(
            HOME,
            "GET",
            &format!("/v2/email/contact-lists/{list}/contacts/{address}"),
            None,
        )
        .await
    }

    /// `SendEmail` from `from` to `recipients`, carrying `options` as its
    /// `ListManagementOptions` when there are any.
    async fn send(
        &self,
        region: &str,
        from: &str,
        recipients: &[&str],
        options: Option<Value>,
    ) -> Reply {
        let mut request = json!({
            "FromEmailAddress": from,
            "Destination": { "ToAddresses": recipients },
            "Content": {"Simple": {
                "Subject": {"Data": "hi"},
                "Body": {"Text": {"Data": "hello"}},
            }},
        });
        if let Some(options) = options {
            request["ListManagementOptions"] = options;
        }
        self.call(region, "POST", "/v2/email/outbound-emails", Some(request))
            .await
    }

    /// Bring every identity's verification transition into the past, so a case that
    /// sends from an identity the account holds does not race the span the clock owes
    /// it.
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
}

/// A store holding the contact list and the verified sending domain the accepted
/// sends use.
async fn seeded() -> Ses {
    let ses = Ses::new();
    let created = ses.create_list(LIST).await;
    assert_eq!(
        created.status, 200,
        "expected the contact list to be created, got {} {:?}",
        created.status, created.body
    );
    let identity = ses.create_identity(SENDER_DOMAIN).await;
    assert_eq!(
        identity.status, 200,
        "expected the sending domain to be created, got {} {:?}",
        identity.status, identity.body
    );
    ses.settle_identities();
    ses
}

#[tokio::test]
async fn a_list_the_account_does_not_hold_is_refused() {
    // Every way the lookup can miss: a name nothing created, one that differs from
    // the stored name only in case (a contact list is keyed exactly as it was
    // created), one that merely contains it, and one with surrounding whitespace.
    // The refusal does not depend on the sender, so each case is sent from the
    // verified sender and from one no identity authorises.
    let missing = [
        "no-such-list",
        "NEWSLETTER",
        "newsletter-2",
        " newsletter",
        "news",
    ];
    for name in missing {
        for from in [SENDER, "stranger@nowhere.example"] {
            let ses = seeded().await;
            let reply = ses
                .send(
                    HOME,
                    from,
                    &["reader@example.net"],
                    Some(json!({"ContactListName": name})),
                )
                .await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (404, "NotFoundException"),
                "case ({name:?}, from {from:?}): expected 404 NotFoundException, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.message(),
                format!("List with name: {name} doesn't exist."),
                "case ({name:?}, from {from:?}): expected the refusal to name the list, got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_topic_the_list_does_not_publish_is_refused() {
    for topic in ["no-such-topic", "WEEKLY", "weekly-digest"] {
        let ses = seeded().await;
        let reply = ses
            .send(
                HOME,
                SENDER,
                &["reader@example.net"],
                Some(json!({"ContactListName": LIST, "TopicName": topic})),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {topic:?}: expected 404 NotFoundException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("List: {LIST} doesn't contain Topic: {topic}"),
            "case {topic:?}: expected the refusal to name the topic and the list, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_well_formed_pair_goes_on_to_the_sender_check() {
    // The list and the topic resolve, so the send proceeds to the gate on the sending
    // identity — and an unverified sender is refused there, in the Region whose
    // identity list the check failed in.
    for region in ["us-east-1", "us-west-2", "eu-central-1"] {
        for options in [
            json!({"ContactListName": LIST}),
            json!({"ContactListName": LIST, "TopicName": TOPIC}),
        ] {
            let ses = seeded().await;
            let reply = ses
                .send(
                    region,
                    "stranger@nowhere.example",
                    &["reader@example.net"],
                    Some(options.clone()),
                )
                .await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "MessageRejected"),
                "case ({region}, {options}): expected 400 MessageRejected, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.message(),
                format!(
                    "Email address is not verified. The following identities failed the check in \
                     region {}: stranger@nowhere.example",
                    region.to_uppercase()
                ),
                "case ({region}, {options}): expected the refusal to name the Region and the sender, got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_send_makes_its_recipients_contacts_of_the_list() {
    // The rows span the recipient counts and the shapes a destination can take, since
    // every address a message is addressed to belongs to the list it was sent to.
    let cases: &[(&str, Vec<&str>)] = &[
        ("one recipient", vec!["reader@example.net"]),
        (
            "two recipients",
            vec!["first@example.net", "second@example.org"],
        ),
        (
            "a recipient whose domain is a subdomain",
            vec!["reader@mail.example.net"],
        ),
        (
            "the same recipient twice",
            vec!["reader@example.net", "reader@example.net"],
        ),
    ];

    for (case, recipients) in cases {
        // What the send states about the contact it creates: a send that names a topic
        // subscribes the contact to that topic, one that names none states nothing (and
        // `GetContact` omits the member entirely rather than answering an empty list).
        for (options, expected_preferences) in [
            (json!({"ContactListName": LIST}), None),
            (
                json!({"ContactListName": LIST, "TopicName": TOPIC}),
                Some(json!([{"TopicName": TOPIC, "SubscriptionStatus": "OPT_IN"}])),
            ),
        ] {
            let ses = seeded().await;
            let reply = ses
                .send(HOME, SENDER, recipients, Some(options.clone()))
                .await;
            assert_eq!(
                reply.status, 200,
                "case ({case:?}, {options}): expected the send to be accepted, got {} {:?}",
                reply.status, reply.body
            );
            assert!(
                reply
                    .body
                    .get("MessageId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()),
                "case ({case:?}, {options}): expected a MessageId, got {:?}",
                reply.body
            );

            let listed = ses.list_contacts(LIST).await;
            assert_eq!(
                listed.status, 200,
                "case ({case:?}, {options}): expected the list's contacts to be readable, got {} {:?}",
                listed.status, listed.body
            );
            let mut expected: Vec<String> =
                recipients.iter().map(|address| (*address).to_string()).collect();
            expected.sort();
            expected.dedup();
            assert_eq!(
                listed.contacts(),
                expected,
                "case ({case:?}, {options}): expected every recipient to be a contact of the list, got {:?}",
                listed.contacts()
            );
            // EVERY recipient the send put on the list carries what the send stated,
            // not just the first one.
            for recipient in recipients {
                let contact = ses.get_contact(LIST, recipient).await;
                assert_eq!(
                    contact.status, 200,
                    "case ({case:?}, {options}, {recipient}): expected the new contact to be readable, got {} {:?}",
                    contact.status, contact.body
                );
                assert_eq!(
                    contact.body.get("TopicPreferences"),
                    expected_preferences.as_ref(),
                    "case ({case:?}, {options}, {recipient}): expected TopicPreferences {expected_preferences:?}, got {:?}",
                    contact.body.get("TopicPreferences")
                );
            }
        }
    }
}

#[tokio::test]
async fn the_topic_a_send_names_is_the_one_the_new_contact_is_subscribed_to() {
    // A list publishing several topics, each with its own default: the subscription the
    // send states is for the topic the MESSAGE named, whatever the list's defaults say
    // and whatever other topics the list publishes. The topic names and the default
    // statuses span the two values `SubscriptionStatus` carries (the enum is
    // ["OPT_IN", "OPT_OUT"]).
    let topics: &[(&str, &str)] = &[
        ("weekly", "OPT_IN"),
        ("monthly", "OPT_OUT"),
        ("Breaking.News_1", "OPT_IN"),
    ];
    let list = "many-topics";

    for (topic, _default_status) in topics {
        let ses = Ses::new();
        let created = ses.create_list_with_topics(list, topics).await;
        assert_eq!(
            created.status, 200,
            "case {topic:?}: expected the contact list to be created, got {} {:?}",
            created.status, created.body
        );
        let identity = ses.create_identity(SENDER_DOMAIN).await;
        assert_eq!(
            identity.status, 200,
            "case {topic:?}: expected the sending domain to be created, got {} {:?}",
            identity.status, identity.body
        );
        ses.settle_identities();

        let recipient = "reader@example.net";
        let reply = ses
            .send(
                HOME,
                SENDER,
                &[recipient],
                Some(json!({"ContactListName": list, "TopicName": topic})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {topic:?}: expected the send to be accepted, got {} {:?}",
            reply.status, reply.body
        );

        let contact = ses.get_contact(list, recipient).await;
        assert_eq!(
            contact.status, 200,
            "case {topic:?}: expected the new contact to be readable, got {} {:?}",
            contact.status, contact.body
        );
        assert_eq!(
            contact.body.get("TopicPreferences"),
            Some(&json!([{"TopicName": topic, "SubscriptionStatus": "OPT_IN"}])),
            "case {topic:?}: expected the contact to be subscribed to the topic the message named, got {:?}",
            contact.body.get("TopicPreferences")
        );
        // The list's own defaults are unchanged by the send: they are the list's, not
        // the contact's. They are reported for the topics the contact stated nothing
        // for, so the topic the message named — which the send subscribed it to — is
        // not among them.
        let defaults: Vec<Value> = topics
            .iter()
            .filter(|(name, _)| name != topic)
            .map(|(topic, default_status)| {
                json!({"TopicName": topic, "SubscriptionStatus": default_status})
            })
            .collect();
        assert_eq!(
            contact.body.get("TopicDefaultPreferences"),
            Some(&Value::Array(defaults)),
            "case {topic:?}: expected the list's own defaults to be reported unchanged, got {:?}",
            contact.body.get("TopicDefaultPreferences")
        );
    }
}

#[tokio::test]
async fn a_recipient_already_on_the_list_is_left_as_it_is() {
    let ses = seeded().await;
    let reader = "reader@example.net";
    let created = ses
        .call(
            HOME,
            "POST",
            &format!("/v2/email/contact-lists/{LIST}/contacts"),
            Some(json!({
                "EmailAddress": reader,
                "TopicPreferences": [{"TopicName": TOPIC, "SubscriptionStatus": "OPT_OUT"}],
                "AttributesData": "{\"tier\":\"gold\"}",
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected the contact to be created, got {} {:?}",
        created.status, created.body
    );
    let before = ses.get_contact(LIST, reader).await;

    let reply = ses
        .send(
            HOME,
            SENDER,
            &[reader, "newcomer@example.net"],
            Some(json!({"ContactListName": LIST, "TopicName": TOPIC})),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected the send to be accepted, got {} {:?}",
        reply.status, reply.body
    );

    let after = ses.get_contact(LIST, reader).await;
    assert_eq!(
        after.body.get("TopicPreferences"),
        before.body.get("TopicPreferences"),
        "expected the contact's stated preferences to be untouched, got {:?} where they were {:?}",
        after.body.get("TopicPreferences"),
        before.body.get("TopicPreferences")
    );
    assert_eq!(
        after.body.get("AttributesData"),
        before.body.get("AttributesData"),
        "expected the contact's attributes to be untouched, got {:?} where they were {:?}",
        after.body.get("AttributesData"),
        before.body.get("AttributesData")
    );
    let listed = ses.list_contacts(LIST).await;
    assert_eq!(
        listed.contacts(),
        vec![
            "newcomer@example.net".to_string(),
            "reader@example.net".to_string()
        ],
        "expected the new recipient to have been added beside the contact that was there, got {:?}",
        listed.contacts()
    );
}

#[tokio::test]
async fn a_send_that_names_no_list_touches_no_list() {
    let ses = seeded().await;
    let reply = ses
        .send(HOME, SENDER, &["reader@example.net"], None)
        .await;
    assert_eq!(
        reply.status, 200,
        "expected the send to be accepted, got {} {:?}",
        reply.status, reply.body
    );
    let listed = ses.list_contacts(LIST).await;
    assert!(
        listed.contacts().is_empty(),
        "expected a send that belongs to no list to add no contact, got {:?}",
        listed.contacts()
    );
}
