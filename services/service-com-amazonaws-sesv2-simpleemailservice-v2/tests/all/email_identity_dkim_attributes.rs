// SPDX-License-Identifier: CC0-1.0

//! `PutEmailIdentityDkimAttributes`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization
//! are all exercised, and the persisted flag is read back through
//! `GetEmailIdentity`.
//!
//! The rule these cases pin: the call stores, on the identity the path names, the
//! DKIM signing setting the request carries — the whole setting, so the stored flag
//! afterwards is exactly the value supplied, whatever it was before and however
//! often the same value is written — and every later read of that identity reports
//! it as `DkimAttributes$SigningEnabled`, while the rest of the DKIM configuration
//! (the authentication status SES derives from DNS, the token set, the origin and
//! the key lengths of the key pair in use) and the identity's other attributes are
//! untouched; an identity the account does not hold is refused with a terminal 404
//! `NotFoundException`. The tables below span what the rule generalises over: both
//! settings, every DKIM configuration an identity can be created with (Easy DKIM at
//! either key length, Bring Your Own DKIM, and an address identity with no DKIM at
//! all), each identity spelling, the orderings and repetitions of writes, the member
//! left out of the request, and each way the lookup can miss.

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

    /// The `DkimAttributes` object the read reported.
    fn dkim(&self) -> Value {
        self.body
            .get("DkimAttributes")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// `DkimAttributes$SigningEnabled`, absent meaning the modelled default `false`.
    fn signing_enabled(&self) -> bool {
        self.dkim()
            .get("SigningEnabled")
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

    /// `CreateEmailIdentity` with whatever body the case supplies, so a case picks
    /// the DKIM configuration the identity starts out with.
    async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identities", Some(body)).await
    }

    async fn get(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn delete(&self, identity: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// `PutEmailIdentityDkimAttributes` with the setting supplied.
    async fn put_dkim(&self, identity: &str, enabled: bool) -> Reply {
        self.put_dkim_body(identity, json!({ "SigningEnabled": enabled }))
            .await
    }

    /// `PutEmailIdentityDkimAttributes` with whatever body the case supplies, so a
    /// case can leave the optional member out entirely.
    async fn put_dkim_body(&self, identity: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/identities/{identity}/dkim"),
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

    /// The DKIM signing flag the handler persisted on `identity`.
    fn stored_signing_enabled(&self, identity: &str) -> bool {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .identities
            .get(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"))
            .dkim
            .signing_enabled
    }
}

/// One identity the rule has to hold for: the name it is created under, and the
/// `CreateEmailIdentity` body that fixes its DKIM configuration.
///
/// The rows span the configurations `CreateEmailIdentity` can produce — Easy DKIM
/// with the default key length, Easy DKIM with the other value of
/// `DkimSigningKeyLength`, Bring Your Own DKIM (the caller's selector and private
/// key), and an email-address identity, for which DKIM is not configured at all —
/// and the domain spellings, including a subdomain and mixed case.
fn identity_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "easy.example.com",
            json!({ "EmailIdentity": "easy.example.com" }),
        ),
        (
            "mail.example.org",
            json!({
                "EmailIdentity": "mail.example.org",
                "DkimSigningAttributes": {"NextSigningKeyLength": "RSA_1024_BIT"},
            }),
        ),
        (
            "Mixed.Case.Example.COM",
            json!({
                "EmailIdentity": "Mixed.Case.Example.COM",
                "DkimSigningAttributes": {"NextSigningKeyLength": "RSA_2048_BIT"},
            }),
        ),
        (
            "byo.example.net",
            json!({
                "EmailIdentity": "byo.example.net",
                "DkimSigningAttributes": {
                    "DomainSigningSelector": "selector7",
                    "DomainSigningPrivateKey": "dGhpcyBpcyBub3QgYSByZWFsIGtleQ==",
                },
            }),
        ),
        (
            "sender@example.com",
            json!({ "EmailIdentity": "sender@example.com" }),
        ),
        (
            "No-Reply@Mail.Example.NET",
            json!({ "EmailIdentity": "No-Reply@Mail.Example.NET" }),
        ),
    ]
}

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
async fn dkim_signing_becomes_the_setting_the_request_carries() {
    for (identity, create_body) in identity_cases() {
        for sequence in WRITE_SEQUENCES {
            let ses = Ses::new();
            let created = ses.create(create_body.clone()).await;
            assert_eq!(
                created.status, 200,
                "case ({identity:?}, {sequence:?}): expected the identity to be created, got {} {:?}",
                created.status, created.body
            );

            for (step, enabled) in sequence.iter().enumerate() {
                let put = ses.put_dkim(identity, *enabled).await;
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
                    ses.stored_signing_enabled(identity),
                    *enabled,
                    "case ({identity:?}, {sequence:?}) step {step}: expected the stored flag to be {enabled:?}, got {:?}",
                    ses.stored_signing_enabled(identity)
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
                read.signing_enabled(),
                last,
                "case ({identity:?}, {sequence:?}): expected DkimAttributes.SigningEnabled {last:?} after the last write, got {:?}",
                read.dkim()
            );
        }
    }
}

#[tokio::test]
async fn a_request_that_omits_the_setting_turns_signing_off() {
    // `SigningEnabled` is modelled with a default of `false`, so a body without it
    // carries the setting `false` — not "leave it as it was".
    for body in [json!({}), json!({ "SigningEnabled": false })] {
        for (identity, create_body) in identity_cases() {
            let ses = Ses::new();
            let created = ses.create(create_body.clone()).await;
            assert_eq!(
                created.status, 200,
                "case ({identity:?}, {body}): expected the identity to be created, got {} {:?}",
                created.status, created.body
            );
            // Only an identity that was created with DKIM configured starts out
            // signing, so the omitted member is an observable change exactly there.
            let signing_at_creation = ses.stored_signing_enabled(identity);

            let put = ses.put_dkim_body(identity, body.clone()).await;
            assert_eq!(
                put.status, 200,
                "case ({identity:?}, {body}): expected 200, got {} {:?}",
                put.status, put.body
            );
            let read = ses.get(identity).await;
            assert!(
                !read.signing_enabled(),
                "case ({identity:?}, {body}): expected DkimAttributes.SigningEnabled false after a request carrying the default (it was {signing_at_creation:?}), got {:?}",
                read.dkim()
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
        ("easy.example.com", "EASY.EXAMPLE.COM"),
        ("Mixed.Case.Example.COM", "mixed.case.example.com"),
        ("sender@example.com", "sender@EXAMPLE.COM"),
        ("No-Reply@Mail.Example.NET", "No-Reply@mail.example.net"),
    ];

    for (created_as, written_as) in cases {
        for enabled in [false, true] {
            let ses = Ses::new();
            ses.create(json!({ "EmailIdentity": created_as })).await;

            let put = ses.put_dkim(written_as, enabled).await;
            assert_eq!(
                put.status, 200,
                "case ({created_as:?}, {written_as:?}, {enabled:?}): expected the write to reach the identity, got {} {:?}",
                put.status, put.body
            );
            let read = ses.get(created_as).await;
            assert_eq!(
                read.signing_enabled(),
                enabled,
                "case ({created_as:?}, {written_as:?}, {enabled:?}): expected DkimAttributes.SigningEnabled {enabled:?}, got {:?}",
                read.dkim()
            );
        }
    }
}

#[tokio::test]
async fn the_rest_of_the_dkim_configuration_is_untouched() {
    for (identity, create_body) in identity_cases() {
        for enabled in [false, true] {
            let ses = Ses::new();
            ses.create(create_body.clone()).await;
            ses.hold_verification_pending(identity);
            let before = ses.get(identity).await;

            let put = ses.put_dkim(identity, enabled).await;
            assert_eq!(
                put.status, 200,
                "case ({identity:?}, {enabled:?}): expected 200, got {} {:?}",
                put.status, put.body
            );

            let after = ses.get(identity).await;
            // The authentication status is what SES finds in the domain's DNS, and
            // the token set, the origin and the key lengths describe the key pair in
            // use; switching signing on or off publishes and revokes nothing, so
            // none of them moves — including the instant SES generated the key pair.
            for member in [
                "Status",
                "Tokens",
                "SigningAttributesOrigin",
                "CurrentSigningKeyLength",
                "NextSigningKeyLength",
                "SigningHostedZone",
                "LastKeyGenerationTimestamp",
            ] {
                assert_eq!(
                    after.dkim().get(member),
                    before.dkim().get(member),
                    "case ({identity:?}, {enabled:?}): expected DkimAttributes.{member} to be unchanged, got {:?} where it was {:?}",
                    after.dkim().get(member),
                    before.dkim().get(member)
                );
            }
            // Nor does anything outside the identity's DKIM configuration.
            for member in [
                "IdentityType",
                "FeedbackForwardingStatus",
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
    ses.create(json!({ "EmailIdentity": "sender@example.com" }))
        .await;
    ses.create(json!({ "EmailIdentity": "example.com" })).await;
    ses.create(json!({ "EmailIdentity": "deleted@example.com" }))
        .await;
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
            let put = ses.put_dkim(identity, enabled).await;
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
    // the DKIM setting they were created with.
    assert!(
        ses.stored_signing_enabled("example.com"),
        "case \"example.com\": expected the domain's stored flag to be unchanged by refused writes, got {:?}",
        ses.stored_signing_enabled("example.com")
    );
    assert!(
        !ses.stored_signing_enabled("sender@example.com"),
        "case \"sender@example.com\": expected the address's stored flag to be unchanged by refused writes, got {:?}",
        ses.stored_signing_enabled("sender@example.com")
    );
}
