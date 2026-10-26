// SPDX-License-Identifier: CC0-1.0

//! `PutEmailIdentityFeedbackAttributes`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization
//! are all exercised, and the persisted flag is read back through
//! `GetEmailIdentity`.
//!
//! The rule these cases pin: the call stores, on the identity the path names, the
//! feedback-forwarding setting the request carries — the whole setting, so the
//! stored flag afterwards is exactly the value supplied, whatever it was before and
//! however often the same value is written — and every later read of that identity
//! reports it, while the identity's other attributes are untouched; an identity the
//! account does not hold is refused with a terminal 404 `NotFoundException`. The
//! tables below span what the rule generalises over: both settings, each identity
//! kind and spelling, the orderings and repetitions of writes, the member left out
//! of the request, and each way the lookup can miss.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, VERIFICATION_SETTLE,
};
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
}

/// The emulated service behind its router, plus the shared engine so a case can
/// read the flag the handler persisted rather than only the read-back response.
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

    async fn create(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": identity })),
        )
        .await
    }

    async fn get(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn delete(&self, identity: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// `PutEmailIdentityFeedbackAttributes` with the setting supplied.
    async fn put_feedback(&self, identity: &str, enabled: bool) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/identities/{identity}/feedback"),
            Some(json!({ "EmailForwardingEnabled": enabled })),
        )
        .await
    }

    /// `PutEmailIdentityFeedbackAttributes` with whatever body the case supplies,
    /// so a case can leave the optional member out entirely.
    async fn put_feedback_body(&self, identity: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/identities/{identity}/feedback"),
            Some(body),
        )
        .await
    }

    /// Hold the clock-owed verification transition off, so a case comparing two
    /// reads of the same identity cannot race the span in which the verification
    /// and DKIM statuses become due.
    fn hold_verification_pending(&self, identity: &str) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        record.verification_due_at =
            SystemTime::now() + VERIFICATION_SETTLE + Duration::from_secs(3600);
    }

    /// The feedback-forwarding flag the handler persisted on `identity`.
    fn stored_feedback_forwarding(&self, identity: &str) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .identities
            .get(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"))
            .feedback_forwarding
    }
}

/// The identity spellings the rule has to hold for: an address, a domain, a
/// subdomain, and the mixed-case forms of each. `IdentityType` is computed from the
/// local part, so both kinds are covered here.
const IDENTITIES: &[&str] = &[
    "sender@example.com",
    "no-reply@mail.example.net",
    "example.com",
    "mail.example.org",
    "Mixed.Case@Example.COM",
    "SUB.Example.ORG",
];

/// One write sequence: the settings written in order, whose LAST value is what every
/// later read must report. The rows span a single write of either setting, both
/// toggle orders, and a value written twice (the same request repeated changes
/// nothing).
const WRITE_SEQUENCES: &[&[bool]] = &[
    &[false],
    &[true],
    &[false, true],
    &[true, false],
    &[false, false],
    &[true, true],
    &[true, false, true],
    &[false, true, false, false],
];

#[tokio::test]
async fn feedback_forwarding_becomes_the_setting_the_request_carries() {
    for identity in IDENTITIES {
        for sequence in WRITE_SEQUENCES {
            let ses = Ses::new();
            let created = ses.create(identity).await;
            assert_eq!(
                created.status, 200,
                "case ({identity:?}, {sequence:?}): expected the identity to be created, got {} {:?}",
                created.status, created.body
            );

            for (step, enabled) in sequence.iter().enumerate() {
                let put = ses.put_feedback(identity, *enabled).await;
                assert_eq!(
                    put.status, 200,
                    "case ({identity:?}, {sequence:?}) step {step}: expected 200, got {} {:?}",
                    put.status, put.body
                );
                // The response shape models no members, so the body carries none.
                assert!(
                    put.body.as_object().is_none_or(serde_json::Map::is_empty),
                    "case ({identity:?}, {sequence:?}) step {step}: the response models no members, got {:?}",
                    put.body
                );
                // Every write is visible immediately, not only the last one.
                assert_eq!(
                    ses.stored_feedback_forwarding(identity),
                    *enabled,
                    "case ({identity:?}, {sequence:?}) step {step}: expected the stored flag to be {enabled:?}, got {:?}",
                    ses.stored_feedback_forwarding(identity)
                );
            }

            let last = *sequence.last().expect("a sequence writes at least once");
            let read = ses.get(identity).await;
            assert_eq!(
                read.status, 200,
                "case ({identity:?}, {sequence:?}): expected the identity to be readable, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.flag("FeedbackForwardingStatus"),
                last,
                "case ({identity:?}, {sequence:?}): expected FeedbackForwardingStatus {last:?} after the last write, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn a_request_that_omits_the_setting_turns_forwarding_off() {
    // `EmailForwardingEnabled` is modelled with a default of `false`, so a body
    // without it carries the setting `false` — not "leave it as it was".
    for body in [json!({}), json!({ "EmailForwardingEnabled": false })] {
        for identity in IDENTITIES {
            let ses = Ses::new();
            ses.create(identity).await;
            // A freshly created identity forwards feedback, so an omitted member
            // that is read as the default is an observable change.
            assert!(
                ses.stored_feedback_forwarding(identity),
                "case ({identity:?}, {body}): expected a new identity to forward feedback, got {:?}",
                ses.stored_feedback_forwarding(identity)
            );

            let put = ses.put_feedback_body(identity, body.clone()).await;
            assert_eq!(
                put.status, 200,
                "case ({identity:?}, {body}): expected 200, got {} {:?}",
                put.status, put.body
            );
            let read = ses.get(identity).await;
            assert!(
                !read.flag("FeedbackForwardingStatus"),
                "case ({identity:?}, {body}): expected FeedbackForwardingStatus false, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn the_domain_is_matched_without_regard_to_case() {
    // DNS compares domain labels case-insensitively, so a write that spells the
    // domain differently reaches the same identity; the local part belongs to the
    // receiving mailbox and is compared as supplied.
    let cases: &[(&str, &str)] = &[
        ("sender@example.com", "sender@EXAMPLE.COM"),
        ("Mixed.Case@Example.COM", "Mixed.Case@example.com"),
        ("example.com", "EXAMPLE.COM"),
        ("SUB.Example.ORG", "sub.example.org"),
    ];

    for (created_as, written_as) in cases {
        let ses = Ses::new();
        ses.create(created_as).await;

        let put = ses.put_feedback(written_as, false).await;
        assert_eq!(
            put.status, 200,
            "case ({created_as:?}, {written_as:?}): expected the write to reach the identity, got {} {:?}",
            put.status, put.body
        );
        let read = ses.get(created_as).await;
        assert!(
            !read.flag("FeedbackForwardingStatus"),
            "case ({created_as:?}, {written_as:?}): expected FeedbackForwardingStatus false, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_identitys_other_attributes_are_untouched() {
    for identity in IDENTITIES {
        for enabled in [false, true] {
            let ses = Ses::new();
            ses.create(identity).await;
            ses.hold_verification_pending(identity);
            let before = ses.get(identity).await;

            let put = ses.put_feedback(identity, enabled).await;
            assert_eq!(
                put.status, 200,
                "case ({identity:?}, {enabled:?}): expected 200, got {} {:?}",
                put.status, put.body
            );

            let after = ses.get(identity).await;
            for member in [
                "IdentityType",
                "DkimAttributes",
                "Policies",
                "Tags",
                "ConfigurationSetName",
                "MailFromAttributes",
                "VerificationInfo",
                // Held pending above, so these two cannot move under the case.
                "VerificationStatus",
                "VerifiedForSendingStatus",
            ] {
                assert_eq!(
                    after.body.get(member),
                    before.body.get(member),
                    "case ({identity:?}, {enabled:?}): expected {member} to be unchanged, got {:?} where it was {:?}",
                    after.body.get(member),
                    before.body.get(member)
                );
            }
        }
    }
}

#[tokio::test]
async fn an_identity_the_account_does_not_hold_is_refused() {
    let ses = Ses::new();
    ses.create("sender@example.com").await;
    ses.create("example.com").await;
    ses.create("deleted@example.com").await;
    let deleted = ses.delete("deleted@example.com").await;
    assert_eq!(
        deleted.status, 200,
        "expected the identity to be deleted, got {} {:?}",
        deleted.status, deleted.body
    );

    // Each way the lookup misses: never created, created and then deleted, a local
    // part differing only in case (the local part is compared as supplied), and a
    // domain that merely contains a stored one.
    let missing: &[&str] = &[
        "absent@example.com",
        "absent.example.com",
        "deleted@example.com",
        "SENDER@example.com",
        "sub.example.com",
    ];

    for identity in missing {
        for enabled in [false, true] {
            let put = ses.put_feedback(identity, enabled).await;
            assert_eq!(
                put.status, 404,
                "case ({identity:?}, {enabled:?}): expected 404, got {} {:?}",
                put.status, put.body
            );
            assert_eq!(
                put.error_code(),
                "NotFoundException",
                "case ({identity:?}, {enabled:?}): expected NotFoundException, got {:?}",
                put.body
            );
            assert_eq!(
                put.message(),
                format!("Email identity {identity} does not exist."),
                "case ({identity:?}, {enabled:?}): expected the message to name the identity, got {:?}",
                put.message()
            );
        }
    }

    // A refused write persists nothing: the identities the account does hold keep
    // the setting they were created with.
    for held in ["sender@example.com", "example.com"] {
        assert!(
            ses.stored_feedback_forwarding(held),
            "case {held:?}: expected the stored flag to be unchanged by refused writes, got {:?}",
            ses.stored_feedback_forwarding(held)
        );
    }
}
