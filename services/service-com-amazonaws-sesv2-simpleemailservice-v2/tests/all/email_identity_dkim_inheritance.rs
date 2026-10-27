// SPDX-License-Identifier: CC0-1.0

//! The DKIM attributes an email-address identity reports, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: DKIM keys are published in a DOMAIN's DNS, so an
//! email-address identity has no DKIM configuration of its own — every read of one
//! reports the configuration of the domain identity that covers it, whatever that
//! configuration is and whenever the read happens, while an address no domain
//! identity covers reports the unconfigured state it was created with.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, VERIFICATION_SETTLE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status and the parsed body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The `DkimAttributes` object the response reported.
    fn dkim(&self) -> Value {
        self.body
            .get("DkimAttributes")
            .cloned()
            .unwrap_or(Value::Null)
    }
}

/// The emulated service behind its router, plus the shared engine so a case can move
/// a stored due instant instead of waiting on the wall clock.
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
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identities", Some(body)).await
    }

    async fn get(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    /// Materialise the verification transition the clock owes `identity`, so the case
    /// reads a domain whose DKIM records have been found.
    async fn settle_verification(&self, identity: &str) {
        {
            let state = self
                .engine
                .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
            let mut store = state.lock();
            let record = store
                .identities
                .get_mut(&identity_key(identity))
                .unwrap_or_else(|| {
                    panic!("case {identity:?}: the identity must exist in the store")
                });
            record.verification_due_at = SystemTime::now()
                .checked_sub(Duration::from_millis(1))
                .expect("an instant just before now");
        }
        let read = self.get(identity).await;
        assert_eq!(
            read.body.get("VerificationStatus").and_then(Value::as_str),
            Some("SUCCESS"),
            "case {identity:?}: expected the verification to have been materialised, got {:?}",
            read.body
        );
    }

    /// Hold the clock-owed verification transition off, so a case that reads a domain
    /// whose records have NOT been found yet cannot race the span.
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
}

/// One covering domain and an address under it: the `CreateEmailIdentity` body the
/// domain is created with, the domain's name, and the address.
///
/// The rows span every DKIM configuration a domain identity can be created with —
/// Easy DKIM at either key length and Bring Your Own DKIM — and the shapes the
/// address can take under it: the domain itself, a subdomain of it, and a spelling
/// whose case differs from the domain identity's (DNS compares labels without regard
/// to case).
fn covered_cases() -> Vec<(&'static str, Value, &'static str, &'static str)> {
    vec![
        (
            "easy DKIM, address in the domain",
            json!({"EmailIdentity": "easy.example.com"}),
            "easy.example.com",
            "sender@easy.example.com",
        ),
        (
            "easy DKIM, address in a subdomain",
            json!({"EmailIdentity": "easy.example.com"}),
            "easy.example.com",
            "sender@mail.easy.example.com",
        ),
        (
            "easy DKIM at the other key length",
            json!({
                "EmailIdentity": "short.example.org",
                "DkimSigningAttributes": {"NextSigningKeyLength": "RSA_1024_BIT"},
            }),
            "short.example.org",
            "sender@short.example.org",
        ),
        (
            "bring your own DKIM",
            json!({
                "EmailIdentity": "byo.example.net",
                "DkimSigningAttributes": {
                    "DomainSigningSelector": "selector7",
                    "DomainSigningPrivateKey": "dGhpcyBpcyBub3QgYSByZWFsIGtleQ==",
                },
            }),
            "byo.example.net",
            "sender@byo.example.net",
        ),
        (
            "the address spells the domain in another case",
            json!({"EmailIdentity": "Mixed.Example.COM"}),
            "Mixed.Example.COM",
            "Sender@MIXED.example.com",
        ),
    ]
}

#[tokio::test]
async fn an_address_reports_the_dkim_attributes_of_the_domain_that_covers_it() {
    for (case, domain_body, domain, address) in covered_cases() {
        for verified in [false, true] {
            let ses = Ses::new();
            let created = ses.create(domain_body.clone()).await;
            assert_eq!(
                created.status, 200,
                "case ({case:?}, verified {verified:?}): expected the domain to be created, got {} {:?}",
                created.status, created.body
            );
            if verified {
                ses.settle_verification(domain).await;
            } else {
                ses.hold_verification_pending(domain);
            }

            // The create of the address answers the covering domain's attributes, and
            // so does every read of it afterwards.
            let child_created = ses.create(json!({"EmailIdentity": address})).await;
            assert_eq!(
                child_created.status, 200,
                "case ({case:?}, verified {verified:?}): expected the address to be created, got {} {:?}",
                child_created.status, child_created.body
            );
            let parent = ses.get(domain).await;
            assert_eq!(
                child_created.dkim(),
                parent.dkim(),
                "case ({case:?}, verified {verified:?}): expected the create of {address:?} to answer the domain's DKIM attributes, got {:?} against {:?}",
                child_created.dkim(),
                parent.dkim()
            );
            let child = ses.get(address).await;
            assert_eq!(
                child.dkim(),
                parent.dkim(),
                "case ({case:?}, verified {verified:?}): expected a read of {address:?} to answer the domain's DKIM attributes, got {:?} against {:?}",
                child.dkim(),
                parent.dkim()
            );
            // The three members a client gates sending on, stated outright for the
            // verified case: a covered address signs with the domain's key.
            if verified {
                assert_eq!(
                    (
                        child.dkim().get("SigningEnabled").and_then(Value::as_bool),
                        child.dkim().get("Status").and_then(Value::as_str),
                    ),
                    (Some(true), Some("SUCCESS")),
                    "case ({case:?}, verified): expected the address to sign with the verified domain's key, got {:?}",
                    child.dkim()
                );
                assert_eq!(
                    child.dkim().get("Tokens"),
                    parent.dkim().get("Tokens"),
                    "case ({case:?}, verified): expected the domain's own token set, got {:?}",
                    child.dkim()
                );
            }
        }
    }
}

#[tokio::test]
async fn an_address_no_domain_identity_covers_reports_its_own_state() {
    // Nothing publishes DKIM records for a bare address, so the identity reports the
    // unconfigured state it was created with. The rows span an address whose domain
    // no identity names, one whose domain merely CONTAINS a domain identity's name,
    // and one whose domain identity is a SUBDOMAIN of the address's domain (covering
    // runs from the address up to the root, never down).
    let ses = Ses::new();
    ses.create(json!({"EmailIdentity": "mail.example.com"})).await;
    ses.create(json!({"EmailIdentity": "notexample.com"})).await;

    for address in [
        "solo@uncovered.example.net",
        "solo@example.com",
        "solo@myexample.com",
    ] {
        let created = ses.create(json!({"EmailIdentity": address})).await;
        assert_eq!(
            created.status, 200,
            "case {address:?}: expected the address to be created, got {} {:?}",
            created.status, created.body
        );
        let read = ses.get(address).await;
        assert_eq!(
            (
                read.dkim().get("SigningEnabled").and_then(Value::as_bool),
                read.dkim().get("Status").and_then(Value::as_str),
                read.dkim().get("Tokens"),
            ),
            (Some(false), Some("NOT_STARTED"), None),
            "case {address:?}: expected the unconfigured DKIM state, got {:?}",
            read.dkim()
        );
        assert_eq!(
            created.dkim(),
            read.dkim(),
            "case {address:?}: expected the create and the read to agree, got {:?} against {:?}",
            created.dkim(),
            read.dkim()
        );
    }
}

#[tokio::test]
async fn a_domain_identity_reports_its_own_dkim_attributes() {
    // A domain publishes its own records, so a domain identity under another domain
    // identity is NOT covered by it: each reports the key pair it was created with,
    // and two domains never share a token set.
    let ses = Ses::new();
    ses.create(json!({"EmailIdentity": "example.com"})).await;
    ses.create(json!({"EmailIdentity": "mail.example.com"})).await;
    ses.hold_verification_pending("example.com");
    ses.hold_verification_pending("mail.example.com");

    let parent = ses.get("example.com").await;
    let child = ses.get("mail.example.com").await;
    assert_ne!(
        parent.dkim().get("Tokens"),
        child.dkim().get("Tokens"),
        "case subdomain-identity: expected each domain to publish its own tokens, got {:?} against {:?}",
        parent.dkim(),
        child.dkim()
    );
}
