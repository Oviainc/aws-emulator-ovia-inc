// SPDX-License-Identifier: CC0-1.0

//! `PutAccountSendingAttributes`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the handler and serialization are all
//! exercised, and what the call persisted is read back through `GetAccount` and
//! through the sending path it governs.
//!
//! The rule these cases pin: the call stores the account-level sending switch the
//! request carries — the whole setting, so the switch afterwards is exactly the
//! value supplied, whatever it was before and however often the same value is
//! written, and a request that omits the member stores the member's modelled
//! default — every later caller sees it (`GetAccount` reports it as
//! `SendingEnabled`, and `SendEmail` refuses while it is off), and nothing else
//! about the account moves. The tables below span what the rule generalises over:
//! both settings, the member supplied and left out, the orderings and repetitions
//! of writes, and the sends either side of each toggle.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    AccountState, ComAmazonawsSesv2SimpleemailserviceV2State, SendQuotaState, FEATURE_ENABLED,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The modelled default of `PutAccountSendingAttributesRequest$SendingEnabled`: the
/// member targets `Enabled`, a boolean with a default of `false`
/// (`smithy_query(query="operation",operation="PutAccountSendingAttributes")`), so a
/// request that omits it asks for sending to be disabled.
const SENDING_ENABLED_DEFAULT: bool = false;

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

    /// A boolean member of the body, absent meaning the modelled default `false`.
    fn flag(&self, member: &str) -> bool {
        self.body
            .get(member)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// This reply's body with `member` dropped, so two reports can be compared on
    /// everything except the one member a case changed.
    fn body_without(&self, member: &str) -> Value {
        let mut body = self.body.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove(member);
        }
        body
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// persist the account state it is about and read the switch the handler kept.
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

    /// `PutAccountSendingAttributes` with whatever body the case supplies, so a case
    /// can leave the optional member out entirely.
    async fn put_sending(&self, body: Value) -> Reply {
        self.call("PUT", "/v2/email/account/sending", Some(body)).await
    }

    /// The account report.
    async fn account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn send(&self, from: &str, to: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/outbound-emails",
            Some(json!({
                "FromEmailAddress": from,
                "Destination": { "ToAddresses": [to] },
                "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}}
            })),
        )
        .await
    }

    /// Persist the account state a case is about.
    fn persist(&self, account: &AccountState) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        store.account = account.clone();
    }

    /// The sending switch the handler persisted.
    fn stored_sending_enabled(&self) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.account.sending_enabled
    }

    /// The recipients currently charged against the account's 24-hour quota.
    fn stored_sent_last_24h(&self) -> f64 {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.send_counters.sent_last_24h
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
}

/// One write of the switch: the request body, and the setting it asks for.
struct SwitchCase {
    name: &'static str,
    body: Value,
    expected: bool,
}

/// The writes the rule has to hold for, in an order that covers the transitions
/// and the repetitions: each setting written from both of its predecessors, each
/// written twice in a row, and the member left out of the request.
fn switch_cases() -> Vec<SwitchCase> {
    vec![
        SwitchCase {
            name: "disable-from-the-default",
            body: json!({ "SendingEnabled": false }),
            expected: false,
        },
        SwitchCase {
            name: "disable-again",
            body: json!({ "SendingEnabled": false }),
            expected: false,
        },
        SwitchCase {
            name: "enable",
            body: json!({ "SendingEnabled": true }),
            expected: true,
        },
        SwitchCase {
            name: "enable-again",
            body: json!({ "SendingEnabled": true }),
            expected: true,
        },
        SwitchCase {
            name: "member-omitted",
            body: json!({}),
            expected: SENDING_ENABLED_DEFAULT,
        },
        SwitchCase {
            name: "enable-after-the-omitted-member",
            body: json!({ "SendingEnabled": true }),
            expected: true,
        },
    ]
}

#[tokio::test]
async fn the_switch_a_request_carries_is_what_every_later_read_reports() {
    let ses = Ses::new();
    for case in switch_cases() {
        let name = case.name;
        let written = ses.put_sending(case.body.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {name:?}: expected 200 from the write, got {} {:?}",
            written.status, written.body
        );
        assert!(
            written.body.is_null() || written.body == json!({}),
            "case {name:?}: expected an empty response body, got {:?}",
            written.body
        );
        assert_eq!(
            ses.stored_sending_enabled(),
            case.expected,
            "case {name:?}: expected the stored switch to be {}, got {:?}",
            case.expected,
            ses.stored_sending_enabled()
        );

        // Every later caller sees it, and a read does not move it: two reports in a
        // row agree with the write.
        for read in ["first", "second"] {
            let reported = ses.account().await;
            assert_eq!(
                reported.status, 200,
                "case {name:?} ({read} read): expected 200 from the account report, got {} {:?}",
                reported.status, reported.body
            );
            assert_eq!(
                reported.flag("SendingEnabled"),
                case.expected,
                "case {name:?} ({read} read): expected SendingEnabled {}, got {:?}",
                case.expected,
                reported.body
            );
        }
    }
}

#[tokio::test]
async fn the_write_leaves_every_other_account_attribute_alone() {
    // An account whose other attributes are all away from their defaults, so a
    // write that reached any of them would show.
    let account = AccountState {
        sending_enabled: true,
        dedicated_ip_auto_warmup_enabled: false,
        production_access_enabled: true,
        vdm_enabled: FEATURE_ENABLED.to_string(),
        vdm_engagement_metrics: Some(FEATURE_ENABLED.to_string()),
        pricing_plan: "ESSENTIALS".to_string(),
        next_pricing_plan: Some("PRO".to_string()),
        suppressed_reasons: Vec::new(),
        ..AccountState::default()
    };

    for (name, asked) in [("to-false", false), ("back-to-true", true)] {
        let ses = Ses::new();
        ses.persist(&account);
        let before = ses.account().await;
        let written = ses.put_sending(json!({ "SendingEnabled": asked })).await;
        assert_eq!(
            written.status, 200,
            "case {name:?}: expected 200 from the write, got {} {:?}",
            written.status, written.body
        );

        let after = ses.account().await;
        assert_eq!(
            after.flag("SendingEnabled"),
            asked,
            "case {name:?}: expected SendingEnabled {asked}, got {:?}",
            after.body
        );
        assert_eq!(
            after.body_without("SendingEnabled"),
            before.body_without("SendingEnabled"),
            "case {name:?}: expected every other reported attribute to be untouched, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn sending_is_refused_while_the_switch_is_off_and_accepted_once_it_is_on() {
    let ses = Ses::new();
    assert_eq!(
        ses.create_identity("switch.example.com").await.status,
        200,
        "expected the identity create to succeed"
    );
    ses.settle_identities();

    // Each row writes the switch, then sends: the send is accepted exactly while
    // the account's ability to send is on, and a refused send charges nothing
    // against the quota.
    let rows: &[(&str, bool)] = &[
        ("on-at-the-start", true),
        ("off", false),
        ("still-off", false),
        ("on-again", true),
        ("off-again", false),
    ];

    let mut accepted = 0.0;
    for (index, (name, enabled)) in rows.iter().enumerate() {
        let written = ses.put_sending(json!({ "SendingEnabled": enabled })).await;
        assert_eq!(
            written.status, 200,
            "case {name:?}: expected 200 from the write, got {} {:?}",
            written.status, written.body
        );

        let sent = ses
            .send(
                "sender@switch.example.com",
                &format!("rcpt-{index}@example.org"),
            )
            .await;
        if *enabled {
            assert_eq!(
                sent.status, 200,
                "case {name:?}: expected the send to be accepted, got {} {:?}",
                sent.status, sent.body
            );
            accepted += 1.0;
        } else {
            assert_eq!(
                (sent.status, sent.error_code().as_str()),
                (400, "SendingPausedException"),
                "case {name:?}: expected a 400 SendingPausedException, got {} {:?}",
                sent.status,
                sent.body
            );
            assert_eq!(
                sent.message(),
                "The message can't be sent because the account's ability to send email is \
                 currently paused.",
                "case {name:?}: expected the paused-account message, got {:?}",
                sent.message()
            );
        }
        assert_eq!(
            ses.stored_sent_last_24h(),
            accepted,
            "case {name:?}: expected {accepted} recipients charged against the quota, got {}",
            ses.stored_sent_last_24h()
        );
    }
}

#[tokio::test]
async fn a_fresh_account_sends_until_a_request_pauses_it() {
    // The switch a new account holds is the one `GetAccount` already reports, so the
    // first write is what changes sending, not the read before it.
    let ses = Ses::new();
    assert_eq!(
        ses.stored_sending_enabled(),
        AccountState::default().sending_enabled,
        "case fresh: expected a new account to hold the default sending switch"
    );
    assert_eq!(
        ses.stored_sent_last_24h(),
        SendQuotaState::default().sent_last_24h,
        "case fresh: expected nothing charged against a new account's quota"
    );

    assert_eq!(
        ses.create_identity("fresh.example.com").await.status,
        200,
        "expected the identity create to succeed"
    );
    ses.settle_identities();
    assert_eq!(
        ses.send("sender@fresh.example.com", "first@example.org")
            .await
            .status,
        200,
        "case fresh: expected a new account to be able to send"
    );

    let written = ses.put_sending(json!({ "SendingEnabled": false })).await;
    assert_eq!(
        written.status, 200,
        "case paused: expected 200 from the write, got {} {:?}",
        written.status, written.body
    );
    let refused = ses
        .send("sender@fresh.example.com", "second@example.org")
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "SendingPausedException"),
        "case paused: expected a 400 SendingPausedException, got {} {:?}",
        refused.status,
        refused.body
    );
}
