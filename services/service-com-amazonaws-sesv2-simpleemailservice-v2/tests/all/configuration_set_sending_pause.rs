// SPDX-License-Identifier: CC0-1.0

//! The per-set sending switch as the SEND path reads it, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised.
//!
//! The rule these cases pin: a configuration set's `SendingOptions$SendingEnabled`
//! gates every message that names that set. While it is off, `SendEmail` naming the
//! set is refused `SendingPausedException` (HTTP 400) in the sentence that names the
//! set, before any `MessageId` is minted and with nothing charged against the
//! account's quota; while it is on, the same request is accepted with a `MessageId`
//! of its own, never a replay of an earlier one. A name the account holds no set
//! under is refused `NotFoundException` (HTTP 404), and a send that names no set, or
//! another set, is not affected by a paused one.
//!
//! The table spans what the rule generalises over: the form of the configuration set
//! name (one character, digits, both separators, the longest a name may be), how the
//! set came to be paused (created paused, created with the `SendingOptions` member
//! present but `SendingEnabled` omitted, paused by a later write), how much other
//! configuration the set carries, and the length of the pause-and-restore sequence
//! each row drives.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, MAX_CONFIGURATION_SET_NAME_LEN,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The domain identity every send in this suite is from.
const SENDER_DOMAIN: &str = "pause.example.com";

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

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `MessageId` a send reports, or `""` when the reply carries none.
    fn message_id(&self) -> String {
        self.body
            .get("MessageId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A boolean at `pointer`, absent reading as `false` — the modelled default of
    /// every boolean on a configuration set.
    fn flag(&self, pointer: &str) -> bool {
        self.body
            .pointer(pointer)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// settle the verification span and read the quota the send path charges.
struct Ses {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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

    async fn create_set(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/configuration-sets", Some(body))
            .await
    }

    async fn get_set(&self, name: &str) -> Reply {
        self.call("GET", &format!("/v2/email/configuration-sets/{name}"), None)
            .await
    }

    /// `PutConfigurationSetSendingOptions`.
    async fn put_set_sending(&self, name: &str, enabled: bool) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{name}/sending"),
            Some(json!({ "SendingEnabled": enabled })),
        )
        .await
    }

    /// `SendEmail`, naming `configuration_set` when the case supplies one.
    async fn send(&self, to: &str, configuration_set: Option<&str>) -> Reply {
        let mut body = json!({
            "FromEmailAddress": format!("sender@{SENDER_DOMAIN}"),
            "Destination": { "ToAddresses": [to] },
            "Content": {"Simple": {
                "Subject": {"Data": "x"},
                "Body": {"Text": {"Data": "y"}},
            }},
        });
        if let Some(name) = configuration_set {
            body["ConfigurationSetName"] = json!(name);
        }
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
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

    /// The recipients currently charged against the account's 24-hour quota.
    fn stored_sent_last_24h(&self) -> f64 {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.send_counters.sent_last_24h
    }

    /// A sending identity for [`SENDER_DOMAIN`], verified past the span the clock
    /// owes it.
    async fn with_verified_sender() -> Self {
        let ses = Self::new();
        let created = ses.create_identity(SENDER_DOMAIN).await;
        assert_eq!(
            created.status, 200,
            "expected the identity create to succeed, got {} {:?}",
            created.status, created.body
        );
        ses.settle_identities();
        ses
    }
}

/// The sentence a send through a paused configuration set is refused with: fixed
/// text plus the set the request named.
fn paused_message(name: &str) -> String {
    format!("Sending is paused for configuration set {name}")
}

/// One row of the table: a configuration set, how it is created, and whether a send
/// through it is accepted right after that create.
struct Case {
    case: &'static str,
    name: String,
    /// The whole `CreateConfigurationSet` body, so a row can carry as much or as
    /// little other configuration as it likes.
    create: Value,
    /// What the set's switch stands at right after the create.
    initially_enabled: bool,
}

/// The rows of the table.
///
/// Built at run time because one row carries the longest name a
/// `ConfigurationSetName` may be.
fn cases() -> Vec<Case> {
    let longest_name = "s".repeat(MAX_CONFIGURATION_SET_NAME_LEN);
    vec![
        Case {
            case: "a set that holds SES's own default, one-character name",
            name: "p".to_string(),
            create: json!({ "ConfigurationSetName": "p" }),
            initially_enabled: true,
        },
        Case {
            case: "a set created paused",
            name: "Paused_Set-1".to_string(),
            create: json!({
                "ConfigurationSetName": "Paused_Set-1",
                "SendingOptions": {"SendingEnabled": false},
            }),
            initially_enabled: false,
        },
        Case {
            // `SendingOptions` present with the member left out asks for the
            // member's modelled default, which is `false`.
            case: "a set created with SendingOptions but no SendingEnabled",
            name: "set9".to_string(),
            create: json!({
                "ConfigurationSetName": "set9",
                "SendingOptions": {},
                "ReputationOptions": {"ReputationMetricsEnabled": true},
            }),
            initially_enabled: false,
        },
        Case {
            case: "a set created enabled, with other configuration, longest name",
            name: longest_name.clone(),
            create: json!({
                "ConfigurationSetName": longest_name,
                "SendingOptions": {"SendingEnabled": true},
                "DeliveryOptions": {"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 300},
            }),
            initially_enabled: true,
        },
    ]
}

/// The switch a read of the stored set reports.
async fn reported_sending_enabled(ses: &Ses, name: &str) -> bool {
    let read = ses.get_set(name).await;
    assert_eq!(
        read.status, 200,
        "case {name:?}: expected 200 from the set read, got {} {:?}",
        read.status, read.body
    );
    read.flag("/SendingOptions/SendingEnabled")
}

#[tokio::test]
async fn a_send_is_accepted_exactly_while_its_configuration_set_allows_sending() {
    for case in cases() {
        let Case {
            case: label,
            name,
            create,
            initially_enabled,
        } = case;
        let ses = Ses::with_verified_sender().await;
        let created = ses.create_set(create).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the set to be created, got {} {:?}",
            created.status, created.body
        );

        // Every setting of the switch the row drives, in order: the one the create
        // left it at, then paused, then restored, then paused again — so both
        // transitions and a repeated setting appear.
        let steps = [
            (None, initially_enabled),
            (Some(false), false),
            (Some(true), true),
            (Some(true), true),
            (Some(false), false),
        ];

        let mut charged = 0.0;
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for (round, (write, enabled)) in steps.into_iter().enumerate() {
            if let Some(asked) = write {
                let written = ses.put_set_sending(&name, asked).await;
                assert_eq!(
                    written.status, 200,
                    "case {label:?} round {round}: expected 200 from the switch write, got {} {:?}",
                    written.status, written.body
                );
            }
            assert_eq!(
                reported_sending_enabled(&ses, &name).await,
                enabled,
                "case {label:?} round {round}: expected the set to report SendingEnabled {enabled}"
            );

            let sent = ses
                .send(&format!("rcpt-{round}@example.org"), Some(&name))
                .await;
            if enabled {
                assert_eq!(
                    sent.status, 200,
                    "case {label:?} round {round}: expected the send to be accepted, got {} {:?}",
                    sent.status, sent.body
                );
                let id = sent.message_id();
                assert!(
                    !id.is_empty(),
                    "case {label:?} round {round}: expected an accepted send to mint a MessageId, \
                     got {:?}",
                    sent.body
                );
                assert!(
                    ids.insert(id.clone()),
                    "case {label:?} round {round}: expected a MessageId of its own, got the \
                     earlier {id:?}"
                );
                charged += 1.0;
            } else {
                assert_eq!(
                    (sent.status, sent.error_code().as_str()),
                    (400, "SendingPausedException"),
                    "case {label:?} round {round}: expected a 400 SendingPausedException, got {} \
                     {:?}",
                    sent.status,
                    sent.body
                );
                assert_eq!(
                    sent.message(),
                    paused_message(&name),
                    "case {label:?} round {round}: expected the paused-set message, got {:?}",
                    sent.message()
                );
                assert!(
                    sent.message_id().is_empty(),
                    "case {label:?} round {round}: a refused send mints no MessageId, got {:?}",
                    sent.body
                );
            }
            // A refused send charges nothing against the account's quota.
            assert_eq!(
                ses.stored_sent_last_24h(),
                charged,
                "case {label:?} round {round}: expected {charged} recipients charged, got {}",
                ses.stored_sent_last_24h()
            );
        }
    }
}

#[tokio::test]
async fn a_paused_set_holds_back_only_the_sends_that_name_it() {
    let ses = Ses::with_verified_sender().await;
    // Two sets, so the pause has a neighbour to leave alone.
    for name in ["held-back", "neighbour_2"] {
        let created = ses.create_set(json!({ "ConfigurationSetName": name })).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected the set to be created, got {} {:?}",
            created.status, created.body
        );
    }
    let paused = ses.put_set_sending("held-back", false).await;
    assert_eq!(
        paused.status, 200,
        "case held-back: expected 200 from the switch write, got {} {:?}",
        paused.status, paused.body
    );

    // The send that names the paused set is the only one refused: one naming the
    // other set and one naming no set at all are accepted, each with its own id.
    let refused = ses.send("rcpt@example.org", Some("held-back")).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "SendingPausedException"),
        "case held-back: expected a 400 SendingPausedException, got {} {:?}",
        refused.status,
        refused.body
    );

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for unaffected in [Some("neighbour_2"), None] {
        let sent = ses.send("rcpt@example.org", unaffected).await;
        assert_eq!(
            sent.status, 200,
            "case {unaffected:?}: expected the send to be accepted, got {} {:?}",
            sent.status, sent.body
        );
        assert!(
            ids.insert(sent.message_id()),
            "case {unaffected:?}: expected a MessageId of its own, got {:?}",
            sent.message_id()
        );
    }
    assert_eq!(
        ses.stored_sent_last_24h(),
        2.0,
        "expected only the two accepted sends charged against the quota, got {}",
        ses.stored_sent_last_24h()
    );

    // Restoring the switch restores sending through that very set.
    let restored = ses.put_set_sending("held-back", true).await;
    assert_eq!(
        restored.status, 200,
        "case restore: expected 200 from the switch write, got {} {:?}",
        restored.status, restored.body
    );
    let sent = ses.send("rcpt@example.org", Some("held-back")).await;
    assert_eq!(
        sent.status, 200,
        "case restore: expected the send to be accepted, got {} {:?}",
        sent.status, sent.body
    );
    assert!(
        ids.insert(sent.message_id()),
        "case restore: expected a MessageId of its own, got {:?}",
        sent.message_id()
    );
}

#[tokio::test]
async fn a_send_naming_a_set_the_account_does_not_hold_is_refused() {
    let ses = Ses::with_verified_sender().await;
    // The names vary along everything the naming rule admits, and none of these
    // sets was ever created; the last one is created and then deleted, so a set
    // that is gone is as absent as one that never existed.
    let deleted = "deleted-set";
    let created = ses
        .create_set(json!({ "ConfigurationSetName": deleted }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {deleted:?}: expected the set to be created, got {} {:?}",
        created.status, created.body
    );
    let dropped = ses
        .call(
            "DELETE",
            &format!("/v2/email/configuration-sets/{deleted}"),
            None,
        )
        .await;
    assert_eq!(
        dropped.status, 200,
        "case {deleted:?}: expected the set to be deleted, got {} {:?}",
        dropped.status, dropped.body
    );

    let absent = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "9".repeat(MAX_CONFIGURATION_SET_NAME_LEN),
        deleted.to_string(),
    ];
    for name in absent {
        let sent = ses.send("rcpt@example.org", Some(&name)).await;
        assert_eq!(
            (sent.status, sent.error_code().as_str()),
            (404, "NotFoundException"),
            "case {name:?}: expected a 404 NotFoundException, got {} {:?}",
            sent.status,
            sent.body
        );
        assert_eq!(
            sent.message(),
            format!("Configuration set <{name}> does not exist."),
            "case {name:?}: expected the not-found message, got {:?}",
            sent.message()
        );
        assert!(
            sent.message_id().is_empty(),
            "case {name:?}: a refused send mints no MessageId, got {:?}",
            sent.body
        );
    }
    assert_eq!(
        ses.stored_sent_last_24h(),
        0.0,
        "expected nothing charged against the quota by refused sends, got {}",
        ses.stored_sent_last_24h()
    );
}
