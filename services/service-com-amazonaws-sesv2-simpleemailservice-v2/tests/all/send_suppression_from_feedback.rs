// SPDX-License-Identifier: CC0-1.0

//! What a send's recipients do with the message, and what the account-level suppression
//! list is written from, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this service
//! registered — and reads the suppression list back through `GetSuppressedDestination`, so
//! routing, deserialization, the handler, the write and the read after it are all
//! exercised.
//!
//! The RULE these cases pin, in two halves:
//!
//! * the list is WRITTEN by what a recipient does with a message. A recipient that
//!   HARD-bounces, and one that complains, is added to the account-level suppression list
//!   with the matching `SuppressionListReason` and the `MessageId` of the message that
//!   caused it — but only for the reasons the account says it suppresses addresses for
//!   (`SuppressionAttributes$SuppressedReasons`), or the ones the configuration set the
//!   send names overrides them with (`SuppressionOptions$SuppressedReasons`); a recipient
//!   that takes the message adds nothing.
//! * the list is CONSULTED before a message is handed on. A send to an address already on
//!   it is still accepted, with a `MessageId` of its own, and teaches the list nothing new:
//!   the entry keeps the message and the reason it was written with.
//!
//! The table spans what the rule generalises over: the recipient's own reaction (every
//! mailbox the simulator defines, a mailbox it does not define, and an ordinary address
//! outside it), the case the recipient is spelled in, the reasons the account suppresses
//! for (neither, each one, both) and whether the configuration set overrides them. The
//! recorded case — a bounce from a mailbox the simulator does not define — is one row of
//! it.
//!
//! The mailbox catalog below is the documented one for the SES mailbox simulator; the rows
//! include mailboxes beyond the one the recorded case used.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::ComAmazonawsSesv2SimpleemailserviceV2State;
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The sender every send is from, and the domain identity that authorises it.
const SENDER: &str = "sender@senders.example.com";
const SENDER_DOMAIN: &str = "senders.example.com";

/// One answered request.
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
}

/// The emulated service behind its router, plus the engine the cases settle an identity's
/// verification on.
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
            .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
            .header("host", format!("email.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIAEMULATOR0000000/20260101/{REGION}/ses/aws4_request, \
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

    /// The account with a verified sending domain, so every send below is authorised.
    async fn given_verified_sender(&self) {
        let created = self
            .call(
                "POST",
                "/v2/email/identities",
                Some(json!({"EmailIdentity": SENDER_DOMAIN})),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "setup: CreateEmailIdentity({SENDER_DOMAIN}) expected 200, got {} {:?}",
            created.status, created.body
        );
        // Bring the verification transition the clock owes the identity into the past, so
        // a send does not race the span.
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

    /// `CreateConfigurationSet`.
    async fn create_set(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": name})),
        )
        .await
    }

    /// `PutConfigurationSetSuppressionOptions`: the reasons this set overrides the
    /// account's with.
    async fn set_suppression_options(&self, name: &str, reasons: &[&str]) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{name}/suppression-options"),
            Some(json!({"SuppressedReasons": reasons})),
        )
        .await
    }

    /// `PutAccountSuppressionAttributes`: the reasons the account suppresses for.
    async fn set_account_reasons(&self, reasons: &[&str]) -> Reply {
        self.call(
            "PUT",
            "/v2/email/account/suppression",
            Some(json!({"SuppressedReasons": reasons})),
        )
        .await
    }

    /// `SendEmail` to one recipient, through `set` when the case names one.
    async fn send(&self, recipient: &str, set: Option<&str>) -> Reply {
        let mut request = json!({
            "FromEmailAddress": SENDER,
            "Destination": {"ToAddresses": [recipient]},
            "Content": {"Simple": {
                "Subject": {"Data": "a message under test"},
                "Body": {"Text": {"Data": "hello"}},
            }},
        });
        if let Some(set) = set {
            request["ConfigurationSetName"] = json!(set);
        }
        self.call("POST", "/v2/email/outbound-emails", Some(request))
            .await
    }

    /// The `MessageId` an accepted send answered.
    async fn send_accepted(&self, case: &str, recipient: &str, set: Option<&str>) -> String {
        let reply = self.send(recipient, set).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the send to {recipient} to be accepted, got {} {} ({})",
            reply.status,
            reply.error_code(),
            reply.message()
        );
        let message_id = reply.text("/MessageId");
        assert!(
            !message_id.is_empty(),
            "case {case:?}: an accepted send must answer a MessageId, got {:?}",
            reply.body
        );
        message_id
    }

    /// `GetSuppressedDestination`.
    async fn get_suppressed(&self, address: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/suppression/addresses/{address}"),
            None,
        )
        .await
    }
}

/// How this service words the read of an address that is not on the list: the fixed text
/// around the address the request named, written out here so the suite pins the wording
/// rather than reading it back from the code under test.
fn not_on_the_list(address: &str) -> String {
    format!("Email address {address} does not exist on your suppression list.")
}

/// Assert that `address` is not on the account's suppression list, with the documented
/// refusal.
async fn assert_not_listed(ses: &Ses, case: &str, address: &str) {
    let reply = ses.get_suppressed(address).await;
    assert_eq!(
        reply.status,
        404,
        "case {case:?}: expected 404 for an address that is not suppressed, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    assert_eq!(
        reply.error_code(),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {} ({})",
        reply.error_code(),
        reply.message()
    );
    assert_eq!(
        reply.message(),
        not_on_the_list(address),
        "case {case:?}: expected the documented not-found sentence, got {:?}",
        reply.message()
    );
}

/// Assert that `address` is on the account's suppression list for `reason`, caused by
/// `message_id`.
async fn assert_listed(ses: &Ses, case: &str, address: &str, reason: &str, message_id: &str) {
    let reply = ses.get_suppressed(address).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected the suppression entry to be readable, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    assert_eq!(
        reply.text("/SuppressedDestination/EmailAddress"),
        address,
        "case {case:?}: the entry must report the address as the send spelled it, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.text("/SuppressedDestination/Reason"),
        reason,
        "case {case:?}: expected Reason {reason:?}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.text("/SuppressedDestination/Attributes/MessageId"),
        message_id,
        "case {case:?}: the entry must name the message that caused it, got {:?}",
        reply.body
    );
    assert!(
        reply
            .body
            .pointer("/SuppressedDestination/LastUpdateTime")
            .is_some(),
        "case {case:?}: the entry must carry the instant it was written, got {:?}",
        reply.body
    );
    assert!(
        reply.body.pointer("/SuppressedDestination/TenantName").is_none(),
        "case {case:?}: an entry on the ACCOUNT's list belongs to no tenant, got {:?}",
        reply.body
    );
}

/// One row of the reaction table: the recipient a send addresses, and the
/// `SuppressionListReason` its reaction puts the address on the account's list for — `None`
/// where the reaction adds nothing.
struct ReactionCase {
    case: &'static str,
    recipient: &'static str,
    suppressed_for: Option<&'static str>,
}

/// The rows: one per reaction the recipient of a send can have.
///
/// The mailboxes are the documented mailbox-simulator catalog — `success`, `bounce`,
/// `ooto`, `complaint` and `suppressionlist` — plus a mailbox of that domain the simulator
/// does not define (which is the recorded case), the same in another spelling, and an
/// ordinary address outside the simulator.
const REACTION_CASES: &[ReactionCase] = &[
    ReactionCase {
        case: "the simulator's accepting mailbox",
        recipient: "success@simulator.amazonses.com",
        suppressed_for: None,
    },
    ReactionCase {
        case: "the simulator's bouncing mailbox",
        recipient: "bounce@simulator.amazonses.com",
        suppressed_for: Some("BOUNCE"),
    },
    ReactionCase {
        case: "a mailbox of the simulator's domain it does not define",
        recipient: "f3a7c19e-not-a-mailbox@simulator.amazonses.com",
        suppressed_for: Some("BOUNCE"),
    },
    ReactionCase {
        case: "an undefined mailbox whose domain is spelled in mixed case",
        recipient: "No-Such-Mailbox@Simulator.AmazonSES.com",
        suppressed_for: Some("BOUNCE"),
    },
    ReactionCase {
        case: "the simulator's out-of-the-office mailbox, which takes the message",
        recipient: "ooto@simulator.amazonses.com",
        suppressed_for: None,
    },
    ReactionCase {
        case: "the simulator's complaining mailbox",
        recipient: "complaint@simulator.amazonses.com",
        suppressed_for: Some("COMPLAINT"),
    },
    ReactionCase {
        case: "the simulator's suppression-list mailbox, which teaches the list nothing",
        recipient: "suppressionlist@simulator.amazonses.com",
        suppressed_for: None,
    },
    ReactionCase {
        case: "an ordinary address outside the simulator, which takes the message",
        recipient: "recipient@receivers.example.com",
        suppressed_for: None,
    },
];

#[tokio::test]
async fn what_a_recipient_does_with_a_message_decides_the_suppression_entry() {
    for case in REACTION_CASES {
        let ses = Ses::new();
        ses.given_verified_sender().await;

        // Before the send the address is on no list, and the read says so in the
        // documented sentence.
        assert_not_listed(&ses, case.case, case.recipient).await;

        let message_id = ses.send_accepted(case.case, case.recipient, None).await;
        match case.suppressed_for {
            Some(reason) => {
                assert_listed(&ses, case.case, case.recipient, reason, &message_id).await;
            }
            None => assert_not_listed(&ses, case.case, case.recipient).await,
        }
    }
}

/// One row of the preference table: the reasons the account suppresses for, the reasons the
/// configuration set overrides them with, the recipient addressed, and the reason the
/// address ends up on the list for.
struct PreferenceCase {
    case: &'static str,
    account_reasons: &'static [&'static str],
    set_reasons: Option<&'static [&'static str]>,
    recipient: &'static str,
    suppressed_for: Option<&'static str>,
}

/// The rows: the same reactions under every combination of preferences.
const PREFERENCE_CASES: &[PreferenceCase] = &[
    PreferenceCase {
        case: "an account that suppresses for neither reason",
        account_reasons: &[],
        set_reasons: None,
        recipient: "bounce@simulator.amazonses.com",
        suppressed_for: None,
    },
    PreferenceCase {
        case: "an account that suppresses for complaints only, addressing a bounce",
        account_reasons: &["COMPLAINT"],
        set_reasons: None,
        recipient: "bounce@simulator.amazonses.com",
        suppressed_for: None,
    },
    PreferenceCase {
        case: "an account that suppresses for complaints only, addressing a complaint",
        account_reasons: &["COMPLAINT"],
        set_reasons: None,
        recipient: "complaint@simulator.amazonses.com",
        suppressed_for: Some("COMPLAINT"),
    },
    PreferenceCase {
        case: "an account that suppresses for bounces only",
        account_reasons: &["BOUNCE"],
        set_reasons: None,
        recipient: "bounce@simulator.amazonses.com",
        suppressed_for: Some("BOUNCE"),
    },
    PreferenceCase {
        case: "an account that suppresses for both reasons",
        account_reasons: &["BOUNCE", "COMPLAINT"],
        set_reasons: None,
        recipient: "cf21-undefined@simulator.amazonses.com",
        suppressed_for: Some("BOUNCE"),
    },
    PreferenceCase {
        case: "a set overriding an account that suppresses for both with complaints only",
        account_reasons: &["BOUNCE", "COMPLAINT"],
        set_reasons: Some(&["COMPLAINT"]),
        recipient: "bounce@simulator.amazonses.com",
        suppressed_for: None,
    },
    PreferenceCase {
        case: "a set overriding an account that suppresses for neither with bounces",
        account_reasons: &[],
        set_reasons: Some(&["BOUNCE"]),
        recipient: "bounce@simulator.amazonses.com",
        suppressed_for: Some("BOUNCE"),
    },
    PreferenceCase {
        case: "a set overriding an account that suppresses for both with neither",
        account_reasons: &["BOUNCE", "COMPLAINT"],
        set_reasons: Some(&[]),
        recipient: "complaint@simulator.amazonses.com",
        suppressed_for: None,
    },
];

#[tokio::test]
async fn the_reasons_the_account_suppresses_for_decide_what_a_reaction_adds() {
    for case in PREFERENCE_CASES {
        let ses = Ses::new();
        ses.given_verified_sender().await;
        let written = ses.set_account_reasons(case.account_reasons).await;
        assert_eq!(
            written.status,
            200,
            "case {:?}: setup PutAccountSuppressionAttributes expected 200, got {} {} ({})",
            case.case,
            written.status,
            written.error_code(),
            written.message()
        );

        let set = case.set_reasons.is_some().then_some("suppression-set");
        if let Some(reasons) = case.set_reasons {
            let created = ses.create_set("suppression-set").await;
            assert_eq!(
                created.status,
                200,
                "case {:?}: setup CreateConfigurationSet expected 200, got {} {} ({})",
                case.case,
                created.status,
                created.error_code(),
                created.message()
            );
            let options = ses.set_suppression_options("suppression-set", reasons).await;
            assert_eq!(
                options.status,
                200,
                "case {:?}: setup PutConfigurationSetSuppressionOptions expected 200, got {} {} \
                 ({})",
                case.case,
                options.status,
                options.error_code(),
                options.message()
            );
        }

        let message_id = ses.send_accepted(case.case, case.recipient, set).await;
        match case.suppressed_for {
            Some(reason) => {
                assert_listed(&ses, case.case, case.recipient, reason, &message_id).await;
            }
            None => assert_not_listed(&ses, case.case, case.recipient).await,
        }
    }
}

#[tokio::test]
async fn a_send_to_an_address_already_on_the_list_is_accepted_and_adds_nothing() {
    let case = "a second send to an address a hard bounce suppressed";
    let ses = Ses::new();
    ses.given_verified_sender().await;
    let recipient = "a91c-undefined@simulator.amazonses.com";

    let first = ses.send_accepted(case, recipient, None).await;
    assert_listed(&ses, case, recipient, "BOUNCE", &first).await;

    // The message is still accepted, and gets an id of its own.
    let second = ses.send_accepted(case, recipient, None).await;
    assert_ne!(
        second, first,
        "case {case:?}: every accepted message gets a unique MessageId"
    );
    // And the entry is left exactly as the bounce wrote it: a send SES did not attempt
    // learned nothing new about the address.
    assert_listed(&ses, case, recipient, "BOUNCE", &first).await;
}

#[tokio::test]
async fn a_send_to_a_deleted_entrys_address_suppresses_it_again() {
    let case = "an address suppressed, taken off the list, and sent to again";
    let ses = Ses::new();
    ses.given_verified_sender().await;
    let recipient = "7b04-undefined@simulator.amazonses.com";

    let first = ses.send_accepted(case, recipient, None).await;
    assert_listed(&ses, case, recipient, "BOUNCE", &first).await;

    let deleted = {
        // `DeleteSuppressedDestination` is an operation of its own and is not
        // implemented yet, so the entry is taken off the list through the store the read
        // answers from — which is what that operation will remove.
        let state = ses
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let key = service_com_amazonaws_sesv2_simpleemailservice_v2::state::suppression_key(
            None, recipient,
        );
        store.suppressed_destinations.remove(&key)
    };
    assert!(
        deleted.is_some(),
        "case {case:?}: the entry the bounce wrote must be the one taken off the list"
    );
    assert_not_listed(&ses, case, recipient).await;

    // Off the list, the address is handed on again, bounces again, and is put back —
    // carrying the message that caused it this time.
    let second = ses.send_accepted(case, recipient, None).await;
    assert_ne!(
        second, first,
        "case {case:?}: every accepted message gets a unique MessageId"
    );
    assert_listed(&ses, case, recipient, "BOUNCE", &second).await;
}
