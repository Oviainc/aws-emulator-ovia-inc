// SPDX-License-Identifier: CC0-1.0

//! The email-identity lifecycle and the outbound send, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised, and reads the engine's own state where a response cannot show
//! what was persisted.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::DKIM_SIGNING_KEY_LENGTH_VALUES;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, SentMessage, MAX_IDENTITIES,
    MAX_SENT_MESSAGES, MAX_STORED_BODY_CHARS, SEND_QUOTA_WINDOW, VERIFICATION_SETTLE,
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
        // A shape name may arrive namespaced (`com.amazonaws.sesv2#NotFound`) or
        // with a trailing URL, as the protocol allows.
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

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router, plus the shared engine so a test can
/// read what a handler persisted.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4.
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

    async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identities", Some(body)).await
    }

    async fn get(&self, identity: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/identities/{identity}"),
            None,
        )
        .await
    }

    async fn delete(&self, identity: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/identities/{identity}"),
            None,
        )
        .await
    }

    async fn list(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/list-identities", Some(body))
            .await
    }

    async fn send(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    /// The identity names a listing reports, in the order it reported them.
    fn listed(reply: &Reply) -> Vec<String> {
        reply
            .body
            .get("EmailIdentities")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        row.get("IdentityName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Move the instant at which `identity`'s verification transition falls due.
    ///
    /// The handler stores a due instant rather than sleeping, so a test drives the
    /// transition by moving that instant instead of waiting on the wall clock —
    /// which is what keeps every assertion about the transient state and about the
    /// settled state independent of how loaded the machine is.
    fn set_verification_due(&self, identity: &str, due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        record.verification_due_at = due_at;
    }

    /// Hold the clock-owed verification transition off, so an assertion about the
    /// transient `PENDING` state cannot race the span.
    fn hold_verification_pending(&self, identity: &str) {
        self.set_verification_due(
            identity,
            SystemTime::now() + VERIFICATION_SETTLE + Duration::from_secs(3600),
        );
    }

    /// The span the handler stored between creating `identity` and its verification
    /// transition falling due.
    fn verification_span(&self, identity: &str) -> Duration {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        let record = store
            .identities
            .get(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        record
            .verification_due_at
            .duration_since(record.created_at)
            .expect("the due instant is at or after the create")
    }

    /// Materialise the verification transition the clock owes `identity`: bring its
    /// due instant into the past, then read the identity back, since a read is what
    /// materialises a transition that is already due.
    async fn settle_verification(&self, identity: &str) {
        self.set_verification_due(
            identity,
            SystemTime::now()
                .checked_sub(Duration::from_millis(1))
                .expect("an instant just before now"),
        );
        let reply = self.get(identity).await;
        assert_eq!(
            reply.text("VerificationStatus"),
            "SUCCESS",
            "case {identity:?}: expected VerificationStatus SUCCESS once the transition fell due, got {:?}",
            reply.body
        );
    }

    /// The outbound-message log, read straight out of the engine.
    fn sent_messages(&self) -> Vec<service_com_amazonaws_sesv2_simpleemailservice_v2::state::SentMessage>
    {
        self.engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>()
            .lock()
            .sent_messages
            .clone()
    }

    /// Recipients charged against the account's 24-hour sending quota.
    fn sent_last_24h(&self) -> f64 {
        self.engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>()
            .lock()
            .send_counters
            .sent_last_24h
    }
}

/// One row of the identity-kind table: what an `EmailIdentity` of this shape is
/// reported as, and the DKIM configuration it is created with.
struct IdentityCase {
    identity: &'static str,
    identity_type: &'static str,
    dkim_signing_enabled: bool,
    dkim_status: &'static str,
    dkim_tokens: usize,
}

/// The rule under test, spanning both `IdentityType` values the model supports
/// (`MANAGED_DOMAIN` is documented as "not supported for email identity types").
/// An identity with a local part is an address, anything else is a domain; Easy
/// DKIM applies to domains only, and publishes three CNAME tokens.
const IDENTITY_CASES: &[IdentityCase] = &[
    IdentityCase {
        identity: "sender@example.com",
        identity_type: "EMAIL_ADDRESS",
        dkim_signing_enabled: false,
        dkim_status: "NOT_STARTED",
        dkim_tokens: 0,
    },
    IdentityCase {
        identity: "no-reply@mail.example.net",
        identity_type: "EMAIL_ADDRESS",
        dkim_signing_enabled: false,
        dkim_status: "NOT_STARTED",
        dkim_tokens: 0,
    },
    IdentityCase {
        identity: "example.com",
        identity_type: "DOMAIN",
        dkim_signing_enabled: true,
        dkim_status: "PENDING",
        dkim_tokens: 3,
    },
    IdentityCase {
        identity: "mail.example.org",
        identity_type: "DOMAIN",
        dkim_signing_enabled: true,
        dkim_status: "PENDING",
        dkim_tokens: 3,
    },
    // Case is not what tells an address from a domain — the local part is — and the
    // store keys an identity by the value it was created with. The domain is one no
    // other row creates an identity for, so this row reports the address identity's
    // OWN (unconfigured) DKIM state: an address under a domain identity reports that
    // domain's instead, which `email_identity_dkim_inheritance` covers.
    IdentityCase {
        identity: "Mixed.Case@Example.ORG",
        identity_type: "EMAIL_ADDRESS",
        dkim_signing_enabled: false,
        dkim_status: "NOT_STARTED",
        dkim_tokens: 0,
    },
    IdentityCase {
        identity: "SUB.Example.ORG",
        identity_type: "DOMAIN",
        dkim_signing_enabled: true,
        dkim_status: "PENDING",
        dkim_tokens: 3,
    },
];

#[tokio::test]
async fn create_computes_the_identity_type_and_dkim_state_from_the_identity() {
    let ses = Ses::new();

    for case in IDENTITY_CASES {
        let created = ses
            .create(json!({
                "EmailIdentity": case.identity,
                "Tags": [{"Key": "env", "Value": "test"}],
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.identity, created.status, created.body
        );
        assert_eq!(
            created.text("IdentityType"),
            case.identity_type,
            "case {:?}: expected IdentityType {}, got {:?}",
            case.identity,
            case.identity_type,
            created.body
        );
        // "Starts the process of verifying an email identity": a brand new
        // identity is not verified, so it cannot send yet.
        assert!(
            !created.flag("VerifiedForSendingStatus"),
            "case {:?}: a new identity must not report itself verified, got {:?}",
            case.identity,
            created.body
        );

        let dkim = created
            .body
            .get("DkimAttributes")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            dkim.get("SigningEnabled").and_then(Value::as_bool).unwrap_or(false),
            case.dkim_signing_enabled,
            "case {:?}: expected DkimAttributes.SigningEnabled {}, got {dkim:?}",
            case.identity,
            case.dkim_signing_enabled
        );
        assert_eq!(
            dkim.get("Status").and_then(Value::as_str).unwrap_or_default(),
            case.dkim_status,
            "case {:?}: expected DkimAttributes.Status {}, got {dkim:?}",
            case.identity,
            case.dkim_status
        );
        let tokens = dkim
            .get("Tokens")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            tokens.len(),
            case.dkim_tokens,
            "case {:?}: expected {} DKIM tokens, got {tokens:?}",
            case.identity,
            case.dkim_tokens
        );
        for token in &tokens {
            let token = token.as_str().unwrap_or_default();
            assert_eq!(
                token.len(),
                32,
                "case {:?}: a DKIM CNAME token is 32 characters, got {token:?}",
                case.identity
            );
        }
        if case.dkim_signing_enabled {
            assert_eq!(
                dkim.get("SigningAttributesOrigin").and_then(Value::as_str),
                Some("AWS_SES"),
                "case {:?}: Easy DKIM is configured by SES, got {dkim:?}",
                case.identity
            );
            assert_eq!(
                dkim.get("CurrentSigningKeyLength").and_then(Value::as_str),
                Some("RSA_2048_BIT"),
                "case {:?}: expected the default Easy DKIM key length, got {dkim:?}",
                case.identity
            );
        }

        // A second create of the same identity is refused.
        let again = ses.create(json!({"EmailIdentity": case.identity})).await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (400, "AlreadyExistsException"),
            "case {:?}: expected 400 AlreadyExistsException on a duplicate create, got {} {:?}",
            case.identity,
            again.status,
            again.body
        );
    }

    // Distinct domains get distinct token sets: a client adds them as CNAME
    // records, so two domains sharing one set would be unusable.
    let first = ses.get("example.com").await;
    let second = ses.get("mail.example.org").await;
    assert_ne!(
        first.body.get("DkimAttributes").and_then(|d| d.get("Tokens")),
        second.body.get("DkimAttributes").and_then(|d| d.get("Tokens")),
        "two domains must not share one Easy DKIM token set: {:?} vs {:?}",
        first.body,
        second.body
    );
}

#[tokio::test]
async fn get_reads_back_exactly_what_create_persisted() {
    let ses = Ses::new();

    for case in IDENTITY_CASES {
        // Per-case tags and configuration set, so a defaulted response cannot pass.
        let configuration_set = format!("cs-{}", case.identity_type.to_lowercase());
        let created = ses
            .create(json!({
                "EmailIdentity": case.identity,
                "ConfigurationSetName": configuration_set,
                "Tags": [
                    {"Key": "env", "Value": "test"},
                    {"Key": "owner", "Value": case.identity},
                ],
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.identity, created.status, created.body
        );
        // The verification transition is the clock's, not this assertion's: hold it
        // off so what follows reads the transient state deterministically.
        ses.hold_verification_pending(case.identity);

        let read = ses.get(case.identity).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetEmailIdentity, got {} {:?}",
            case.identity, read.status, read.body
        );
        assert_eq!(
            read.text("IdentityType"),
            case.identity_type,
            "case {:?}: expected IdentityType {}, got {:?}",
            case.identity,
            case.identity_type,
            read.body
        );
        assert_eq!(
            read.text("VerificationStatus"),
            "PENDING",
            "case {:?}: verification has been started and not completed, got {:?}",
            case.identity,
            read.body
        );
        assert_eq!(
            read.text("ConfigurationSetName"),
            configuration_set,
            "case {:?}: expected the stored ConfigurationSetName {configuration_set}, got {:?}",
            case.identity,
            read.body
        );
        assert_eq!(
            read.body.get("Tags").cloned().unwrap_or(Value::Null),
            json!([
                {"Key": "env", "Value": "test"},
                {"Key": "owner", "Value": case.identity},
            ]),
            "case {:?}: expected the stored tags, got {:?}",
            case.identity,
            read.body
        );
        // SES forwards bounces and complaints until a caller turns it off.
        assert!(
            read.flag("FeedbackForwardingStatus"),
            "case {:?}: expected FeedbackForwardingStatus true, got {:?}",
            case.identity,
            read.body
        );
        assert!(
            !read.flag("VerifiedForSendingStatus"),
            "case {:?}: expected VerifiedForSendingStatus false while PENDING, got {:?}",
            case.identity,
            read.body
        );
        // The DKIM view a create reports and the one a read reports are the same
        // stored configuration.
        assert_eq!(
            read.body.get("DkimAttributes"),
            created.body.get("DkimAttributes"),
            "case {:?}: GetEmailIdentity must report the DKIM attributes CreateEmailIdentity returned: {:?} vs {:?}",
            case.identity,
            created.body,
            read.body
        );

        // The listing carries the same computed facts.
        let listed = ses.list(json!({})).await;
        let row = listed
            .body
            .get("EmailIdentities")
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row.get("IdentityName").and_then(Value::as_str) == Some(case.identity))
                    .cloned()
            })
            .unwrap_or(Value::Null);
        assert_eq!(
            row.get("IdentityType").and_then(Value::as_str),
            Some(case.identity_type),
            "case {:?}: expected the listing to report IdentityType {}, got {row:?}",
            case.identity,
            case.identity_type
        );
        assert_eq!(
            row.get("VerificationStatus").and_then(Value::as_str),
            Some("PENDING"),
            "case {:?}: expected the listing to report VerificationStatus PENDING, got {row:?}",
            case.identity
        );
        assert!(
            !row.get("SendingEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "case {:?}: expected the listing to report SendingEnabled false, got {row:?}",
            case.identity
        );
    }
}

#[tokio::test]
async fn delete_removes_the_identity_from_every_reader() {
    let ses = Ses::new();

    for case in IDENTITY_CASES {
        assert_eq!(
            ses.create(json!({"EmailIdentity": case.identity})).await.status,
            200,
            "case {:?}: expected the create to succeed",
            case.identity
        );
    }

    for case in IDENTITY_CASES {
        let deleted = ses.delete(case.identity).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected 200 from DeleteEmailIdentity, got {} {:?}",
            case.identity, deleted.status, deleted.body
        );

        let read = ses.get(case.identity).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected 404 NotFoundException after the delete, got {} {:?}",
            case.identity,
            read.status,
            read.body
        );

        let again = ses.delete(case.identity).await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected 404 NotFoundException deleting an unknown identity, got {} {:?}",
            case.identity,
            again.status,
            again.body
        );

        let listed = ses.list(json!({})).await;
        assert!(
            !Ses::listed(&listed).contains(&case.identity.to_string()),
            "case {:?}: a deleted identity must not be listed, got {:?}",
            case.identity,
            listed.body
        );
    }

    let listed = ses.list(json!({})).await;
    assert_eq!(
        Ses::listed(&listed),
        Vec::<String>::new(),
        "every identity was deleted, so the listing must be empty, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn dkim_signing_attributes_select_the_configuration() {
    let ses = Ses::new();

    // Easy DKIM with an explicitly requested key length. `DkimSigningKeyLength`
    // has exactly two values; both are exercised (the default is covered above).
    let created = ses
        .create(json!({
            "EmailIdentity": "easy.example.com",
            "DkimSigningAttributes": {"NextSigningKeyLength": "RSA_1024_BIT"},
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case easy-dkim-1024: expected 200, got {} {:?}",
        created.status, created.body
    );
    let dkim = created.body.get("DkimAttributes").cloned().unwrap_or(Value::Null);
    assert_eq!(
        dkim.get("CurrentSigningKeyLength").and_then(Value::as_str),
        Some("RSA_1024_BIT"),
        "case easy-dkim-1024: expected the requested key length, got {dkim:?}"
    );
    assert_eq!(
        dkim.get("SigningAttributesOrigin").and_then(Value::as_str),
        Some("AWS_SES"),
        "case easy-dkim-1024: expected origin AWS_SES, got {dkim:?}"
    );

    // BYODKIM: "this object contains the selector for the public key".
    let created = ses
        .create(json!({
            "EmailIdentity": "byo.example.com",
            "DkimSigningAttributes": {
                "DomainSigningSelector": "selector1",
                "DomainSigningPrivateKey": "dGhpcyBpcyBub3QgYSByZWFsIGtleQ==",
            },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case byodkim: expected 200, got {} {:?}",
        created.status, created.body
    );
    let dkim = created.body.get("DkimAttributes").cloned().unwrap_or(Value::Null);
    assert_eq!(
        dkim.get("SigningAttributesOrigin").and_then(Value::as_str),
        Some("EXTERNAL"),
        "case byodkim: expected origin EXTERNAL, got {dkim:?}"
    );
    assert_eq!(
        dkim.get("Tokens").cloned().unwrap_or(Value::Null),
        json!(["selector1"]),
        "case byodkim: expected the selector as the token set, got {dkim:?}"
    );
    assert!(
        dkim.get("CurrentSigningKeyLength").is_none(),
        "case byodkim: the caller owns the key, so SES reports no key length, got {dkim:?}"
    );
    // The private key is @sensitive: it must not come back on the wire, and it is
    // not persisted either.
    let read = ses.get("byo.example.com").await;
    assert!(
        !serde_json::to_string(&read.body)
            .unwrap_or_default()
            .contains("dGhpcyBpcyBub3QgYSByZWFsIGtleQ=="),
        "case byodkim: the DKIM private key must never be echoed back, got {:?}",
        read.body
    );

    // Refusals. Each row is a request the documented rules do not allow.
    let refusals: &[(&str, Value, &str)] = &[
        (
            // "You can only specify this object if the email identity is a domain,
            // as opposed to an address."
            "dkim-attributes-on-an-address",
            json!({
                "EmailIdentity": "someone@refuse.example.com",
                "DkimSigningAttributes": {"NextSigningKeyLength": "RSA_2048_BIT"},
            }),
            "BadRequestException",
        ),
        (
            "byodkim-without-the-private-key",
            json!({
                "EmailIdentity": "half.example.com",
                "DkimSigningAttributes": {"DomainSigningSelector": "selector1"},
            }),
            "BadRequestException",
        ),
        (
            // For CreateEmailIdentity the allowed origins are "All values except
            // AWS_SES and EXTERNAL".
            "origin-aws-ses",
            json!({
                "EmailIdentity": "origin.example.com",
                "DkimSigningAttributes": {"DomainSigningAttributesOrigin": "AWS_SES"},
            }),
            "BadRequestException",
        ),
        (
            "origin-external",
            json!({
                "EmailIdentity": "origin2.example.com",
                "DkimSigningAttributes": {"DomainSigningAttributesOrigin": "EXTERNAL"},
            }),
            "BadRequestException",
        ),
        (
            // A key length outside `DkimSigningKeyLength` names no key SES can
            // generate (one past the published pair below).
            "key-length-outside-the-enum",
            json!({
                "EmailIdentity": "keylen.example.com",
                "DkimSigningAttributes": {"NextSigningKeyLength": "RSA_4096_BIT"},
            }),
            "BadRequestException",
        ),
    ];
    for (case, request, expected) in refusals {
        let reply = ses.create(request.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, *expected),
            "case {case:?}: expected 400 {expected}, got {} {:?} ({})",
            reply.status,
            reply.body,
            reply.message()
        );
    }

    // Cross-Region replication (`AWS_SES_<REGION>`) is not written: it reads a
    // parent identity this emulator does not hold, so it answers 501 rather than
    // inventing one.
    let replicated = ses
        .create(json!({
            "EmailIdentity": "deed.example.com",
            "DkimSigningAttributes": {"DomainSigningAttributesOrigin": "AWS_SES_US_EAST_1"},
        }))
        .await;
    assert_eq!(
        replicated.status, 501,
        "case deed-replication: expected 501, got {} {:?}",
        replicated.status, replicated.body
    );

    // `NextSigningKeyLength` is an Easy DKIM member: with a caller-owned key pair
    // there is no key for SES to generate, so the member has nothing to apply to and
    // the identity is still a BYODKIM one that reports no key length.
    let created = ses
        .create(json!({
            "EmailIdentity": "byokeylen.example.com",
            "DkimSigningAttributes": {
                "DomainSigningSelector": "selector2",
                "DomainSigningPrivateKey": "dGhpcyBpcyBub3QgYSByZWFsIGtleQ==",
                "NextSigningKeyLength": "RSA_2048_BIT",
            },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case byodkim-with-a-key-length: expected 200, got {} {:?}",
        created.status, created.body
    );
    let dkim = created.body.get("DkimAttributes").cloned().unwrap_or(Value::Null);
    assert_eq!(
        dkim.get("SigningAttributesOrigin").and_then(Value::as_str),
        Some("EXTERNAL"),
        "case byodkim-with-a-key-length: expected origin EXTERNAL, got {dkim:?}"
    );
    assert!(
        dkim.get("CurrentSigningKeyLength").is_none(),
        "case byodkim-with-a-key-length: the caller owns the key, so SES reports no key length, got {dkim:?}"
    );

    // Every value the model's `DkimSigningKeyLength` enum publishes names a key SES
    // generates, and the one SES used comes back as `CurrentSigningKeyLength`. The
    // value set is the generated const, so the table cannot drift from the model.
    for (index, length) in DKIM_SIGNING_KEY_LENGTH_VALUES.iter().enumerate() {
        let identity = format!("keylength-{index}.example.com");
        let created = ses
            .create(json!({
                "EmailIdentity": identity,
                "DkimSigningAttributes": {"NextSigningKeyLength": length},
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {length:?}: expected 200, got {} {:?}",
            created.status, created.body
        );
        let read = ses.get(&identity).await;
        assert_eq!(
            read.body
                .get("DkimAttributes")
                .and_then(|dkim| dkim.get("CurrentSigningKeyLength"))
                .and_then(Value::as_str),
            Some(*length),
            "case {length:?}: expected the stored key length to come back, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn listing_pages_through_every_identity() {
    let ses = Ses::new();

    let identities = [
        "a.example.com",
        "b.example.com",
        "c.example.com",
        "d@example.net",
        "e@example.net",
    ];
    for identity in identities {
        assert_eq!(
            ses.create(json!({"EmailIdentity": identity})).await.status,
            200,
            "case {identity:?}: expected the create to succeed"
        );
    }
    let mut expected: Vec<String> = identities.iter().map(|name| name.to_string()).collect();
    expected.sort();

    // The rule generalises over the page size: every size walks the same identity
    // set in the same order, in pages no larger than it asked for.
    for page_size in [1_usize, 2, 3, 5, 9] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"PageSize": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let reply = ses.list(request).await;
            assert_eq!(
                reply.status, 200,
                "case page-size-{page_size}: expected 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = Ses::listed(&reply);
            assert!(
                page.len() <= page_size,
                "case page-size-{page_size}: a page must not exceed the requested size, got {page:?}"
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= identities.len() + 1,
                "case page-size-{page_size}: paging did not terminate, collected {collected:?}"
            );
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case page-size-{page_size}: paging must enumerate every identity exactly once, in order"
        );
    }

    // No PageSize: one page, everything on it, no token.
    let all = ses.list(json!({})).await;
    assert_eq!(
        Ses::listed(&all),
        expected,
        "case no-page-size: expected every identity on one page, got {:?}",
        all.body
    );
    assert!(
        all.body.get("NextToken").is_none(),
        "case no-page-size: a complete listing carries no NextToken, got {:?}",
        all.body
    );

    // `PageSize` 0 is inside the documented range ("at least 0") but the docs do
    // not say what it means; this emulator reads it as "no truncation", the same
    // answer as omitting the member, which is what this row pins.
    let zero = ses.list(json!({"PageSize": 0})).await;
    assert_eq!(
        zero.status, 200,
        "case page-size-0: 0 is inside the documented range, got {} {:?}",
        zero.status, zero.body
    );
    assert_eq!(
        Ses::listed(&zero),
        expected,
        "case page-size-0: expected the whole listing, got {:?}",
        zero.body
    );
    assert!(
        zero.body.get("NextToken").is_none(),
        "case page-size-0: a complete listing carries no NextToken, got {:?}",
        zero.body
    );

    // "The value you specify has to be at least 0, and can be no more than 1000":
    // both published bounds are accepted, and one past each is refused.
    for page_size in [0, 1000] {
        let reply = ses.list(json!({"PageSize": page_size})).await;
        assert_eq!(
            reply.status, 200,
            "case page-size-{page_size}: a published bound must be accepted, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            Ses::listed(&reply),
            expected,
            "case page-size-{page_size}: expected the whole set, got {:?}",
            reply.body
        );
    }
    for page_size in [-1, 1001] {
        let reply = ses.list(json!({"PageSize": page_size})).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case page-size-{page_size}: a value past the published bound must be refused, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // A token this operation did not issue is not a position in the listing —
    // whether it is not base64 at all, or base64 of something this operation never
    // encoded.
    for (case, token) in [
        ("not-base64", "not-a-token".to_string()),
        (
            "base64-of-a-foreign-token",
            aws_smithy_types::base64::encode("b.example.com"),
        ),
        (
            "base64-of-another-service-s-token",
            aws_smithy_types::base64::encode("other-service:things:b.example.com"),
        ),
    ] {
        let reply = ses.list(json!({"NextToken": token})).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

#[tokio::test]
async fn listing_honours_every_documented_filter_key() {
    let ses = Ses::new();

    for identity in ["alpha.example.com", "beta.example.com", "gamma@example.net"] {
        assert_eq!(
            ses.create(json!({"EmailIdentity": identity})).await.status,
            200,
            "case {identity:?}: expected the create to succeed"
        );
        // The VERIFICATION_STATUS cases below filter on the transient state, so the
        // clock-owed transition is held off rather than raced.
        ses.hold_verification_pending(identity);
    }

    // Every `IdentityFilterKey` the model declares, including one value no
    // identity here carries.
    let cases: &[(&str, Value, Vec<&str>)] = &[
        (
            "identity-type-domain",
            json!({"Filter": {"IDENTITY_TYPE": "DOMAIN"}}),
            vec!["alpha.example.com", "beta.example.com"],
        ),
        (
            "identity-type-email-address",
            json!({"Filter": {"IDENTITY_TYPE": "EMAIL_ADDRESS"}}),
            vec!["gamma@example.net"],
        ),
        (
            "identity-type-managed-domain",
            json!({"Filter": {"IDENTITY_TYPE": "MANAGED_DOMAIN"}}),
            vec![],
        ),
        (
            "name-contains-a-substring",
            json!({"Filter": {"IDENTITY_NAME_CONTAINS": "example.com"}}),
            vec!["alpha.example.com", "beta.example.com"],
        ),
        (
            "name-contains-nothing-matching",
            json!({"Filter": {"IDENTITY_NAME_CONTAINS": "zeta"}}),
            vec![],
        ),
        (
            "verification-status-pending",
            json!({"Filter": {"VERIFICATION_STATUS": "PENDING"}}),
            vec!["alpha.example.com", "beta.example.com", "gamma@example.net"],
        ),
        (
            "verification-status-failed",
            json!({"Filter": {"VERIFICATION_STATUS": "FAILED"}}),
            vec![],
        ),
        (
            "two-filters-are-anded",
            json!({"Filter": {"IDENTITY_TYPE": "DOMAIN", "IDENTITY_NAME_CONTAINS": "alpha"}}),
            vec!["alpha.example.com"],
        ),
    ];
    for (case, request, expected) in cases {
        let reply = ses.list(request.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            Ses::listed(&reply),
            expected.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {case:?}: unexpected listing, got {:?}",
            reply.body
        );
    }

    // A filter value outside the member's own enum cannot match anything.
    let reply = ses
        .list(json!({"Filter": {"IDENTITY_TYPE": "NOT_A_TYPE"}}))
        .await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "BadRequestException"),
        "case invalid-filter-value: expected 400 BadRequestException, got {} {:?}",
        reply.status,
        reply.body
    );
}

#[tokio::test]
async fn an_identity_becomes_verified_once_the_transition_falls_due() {
    let ses = Ses::new();

    let created = ses
        .create(json!({"EmailIdentity": "settle@example.com"}))
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from the create, got {} {:?}",
        created.status, created.body
    );

    // The transition is owed by the clock at a stored instant one settle span after
    // the create, and the span is never zero — so a caller acting straight after the
    // create still sees the transient state.
    let span = ses.verification_span("settle@example.com");
    assert_eq!(
        span, VERIFICATION_SETTLE,
        "case span: expected the transition to fall due {VERIFICATION_SETTLE:?} after the create, got {span:?}"
    );
    assert!(
        span > Duration::ZERO,
        "case span: the transition must not be due at the instant of the create, got {span:?}"
    );
    ses.hold_verification_pending("settle@example.com");

    // Before it falls due the identity is still in the transient state, and a send
    // from it is refused for exactly that reason.
    let read = ses.get("settle@example.com").await;
    assert_eq!(
        read.text("VerificationStatus"),
        "PENDING",
        "case before-due: expected PENDING immediately after the create, got {:?}",
        read.body
    );
    let rejected = ses
        .send(json!({
            "FromEmailAddress": "settle@example.com",
            "Destination": {"ToAddresses": ["rcpt@example.net"]},
            "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}},
        }))
        .await;
    assert_eq!(
        (rejected.status, rejected.error_code().as_str()),
        (400, "MessageRejected"),
        "case before-due: an unverified identity cannot send, got {} {:?}",
        rejected.status,
        rejected.body
    );

    ses.settle_verification("settle@example.com").await;

    let read = ses.get("settle@example.com").await;
    assert!(
        read.flag("VerifiedForSendingStatus"),
        "case after-due: expected VerifiedForSendingStatus true, got {:?}",
        read.body
    );
    let listed = ses.list(json!({})).await;
    let row = listed
        .body
        .get("EmailIdentities")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first().cloned())
        .unwrap_or(Value::Null);
    assert_eq!(
        row.get("SendingEnabled").and_then(Value::as_bool),
        Some(true),
        "case after-due: expected the listing to report SendingEnabled true, got {row:?}"
    );
}

#[tokio::test]
async fn send_records_the_message_and_mints_a_unique_id() {
    let ses = Ses::new();

    // A verified domain identity authorises every address in the domain and in its
    // subdomains; a verified address identity authorises itself.
    for identity in ["example.com", "solo@example.net"] {
        assert_eq!(
            ses.create(json!({"EmailIdentity": identity})).await.status,
            200,
            "case {identity:?}: expected the create to succeed"
        );
    }
    ses.settle_verification("example.com").await;
    ses.settle_verification("solo@example.net").await;

    struct SendCase {
        case: &'static str,
        from: &'static str,
        to: &'static [&'static str],
        cc: &'static [&'static str],
        subject: &'static str,
        body: &'static str,
    }
    let cases: &[SendCase] = &[
        SendCase {
            case: "address-identity",
            from: "solo@example.net",
            to: &["rcpt@example.org"],
            cc: &[],
            subject: "hi",
            body: "hello",
        },
        SendCase {
            case: "domain-identity-authorises-any-address",
            from: "anyone@example.com",
            to: &["one@example.org", "two@example.org"],
            cc: &["carbon@example.org"],
            subject: "a longer subject line",
            body: "a body with\nseveral lines",
        },
        SendCase {
            case: "domain-identity-authorises-a-subdomain",
            from: "bot@mail.example.com",
            to: &["three@example.org"],
            cc: &[],
            subject: "",
            body: "",
        },
    ];

    let mut ids: Vec<String> = Vec::new();
    let mut charged = 0.0_f64;
    for case in cases {
        let reply = ses
            .send(json!({
                "FromEmailAddress": case.from,
                "Destination": {"ToAddresses": case.to, "CcAddresses": case.cc},
                "ReplyToAddresses": ["reply@example.net"],
                "FeedbackForwardingEmailAddress": "bounces@example.net",
                "EmailTags": [{"Name": "campaign", "Value": case.case}],
                "Content": {"Simple": {
                    "Subject": {"Data": case.subject},
                    "Body": {"Text": {"Data": case.body}},
                }},
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );
        let message_id = reply.text("MessageId");
        assert!(
            !message_id.is_empty(),
            "case {:?}: expected a MessageId, got {:?}",
            case.case,
            reply.body
        );
        assert!(
            !ids.contains(&message_id),
            "case {:?}: MessageId {message_id} was already issued ({ids:?})",
            case.case
        );
        ids.push(message_id.clone());

        // The message is in the log, with what the request carried.
        let log = ses.sent_messages();
        let stored = log
            .iter()
            .find(|message| message.message_id == message_id)
            .unwrap_or_else(|| panic!("case {:?}: the message was not persisted", case.case));
        assert_eq!(
            stored.from.as_deref(),
            Some(case.from),
            "case {:?}: expected the stored sender {}, got {:?}",
            case.case,
            case.from,
            stored
        );
        assert_eq!(
            stored.to,
            case.to.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            "case {:?}: unexpected stored ToAddresses, got {:?}",
            case.case,
            stored
        );
        assert_eq!(
            stored.cc,
            case.cc.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            "case {:?}: unexpected stored CcAddresses, got {:?}",
            case.case,
            stored
        );
        assert_eq!(
            stored.subject.as_deref(),
            Some(case.subject),
            "case {:?}: unexpected stored subject, got {:?}",
            case.case,
            stored
        );
        assert_eq!(
            stored.body.as_deref(),
            Some(case.body),
            "case {:?}: unexpected stored body, got {:?}",
            case.case,
            stored
        );
        assert_eq!(
            stored.email_tags,
            vec![("campaign".to_string(), case.case.to_string())],
            "case {:?}: unexpected stored EmailTags, got {:?}",
            case.case,
            stored
        );
        assert_eq!(
            stored.reply_to,
            vec!["reply@example.net".to_string()],
            "case {:?}: unexpected stored ReplyToAddresses, got {:?}",
            case.case,
            stored
        );
        assert_eq!(
            stored.feedback_forwarding.as_deref(),
            Some("bounces@example.net"),
            "case {:?}: expected the nominated FeedbackForwardingEmailAddress to be recorded, got {:?}",
            case.case,
            stored
        );

        // Each recipient counts as one message against the sending quota.
        charged += (case.to.len() + case.cc.len()) as f64;
        assert_eq!(
            ses.sent_last_24h(),
            charged,
            "case {:?}: expected {charged} recipients charged against the 24-hour quota",
            case.case
        );
    }

    assert_eq!(
        ses.sent_messages().len(),
        cases.len(),
        "the log must hold one entry per accepted message"
    );

    // A raw message is recorded as sent. `RawMessage$Data` is a blob, which
    // restJson1 carries base64-encoded.
    let raw = "From: solo@example.net\r\nSubject: raw\r\n\r\nraw body\r\n";
    let reply = ses
        .send(json!({
            "FromEmailAddress": "solo@example.net",
            "Destination": {"ToAddresses": ["rcpt@example.org"]},
            "Content": {"Raw": {"Data": aws_smithy_types::base64::encode(raw)}},
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "case raw-content: expected 200, got {} {:?}",
        reply.status, reply.body
    );
    let message_id = reply.text("MessageId");
    let log = ses.sent_messages();
    let stored = log
        .iter()
        .find(|message| message.message_id == message_id)
        .expect("case raw-content: the message was not persisted");
    assert_eq!(
        stored.body.as_deref(),
        Some(raw),
        "case raw-content: expected the raw MIME document to be recorded as sent, got {stored:?}"
    );

    // A simple message may carry a text part, an HTML part or both, and the log
    // records each part the request carried.
    let body_cases: &[(&str, Value, Option<&str>, Option<&str>)] = &[
        (
            "text-only",
            json!({"Text": {"Data": "text part"}}),
            Some("text part"),
            None,
        ),
        (
            "html-only",
            json!({"Html": {"Data": "<p>html part</p>"}}),
            None,
            Some("<p>html part</p>"),
        ),
        (
            "both-parts",
            json!({"Text": {"Data": "text part"}, "Html": {"Data": "<p>html part</p>"}}),
            Some("text part"),
            Some("<p>html part</p>"),
        ),
    ];
    for (case, body, expected_text, expected_html) in body_cases {
        let reply = ses
            .send(json!({
                "FromEmailAddress": "solo@example.net",
                "Destination": {"ToAddresses": ["rcpt@example.org"]},
                "Content": {"Simple": {"Subject": {"Data": case}, "Body": body}},
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        let message_id = reply.text("MessageId");
        let log = ses.sent_messages();
        let stored = log
            .iter()
            .find(|message| message.message_id == message_id)
            .unwrap_or_else(|| panic!("case {case:?}: the message was not persisted"));
        assert_eq!(
            stored.body.as_deref(),
            *expected_text,
            "case {case:?}: unexpected stored text part, got {stored:?}"
        );
        assert_eq!(
            stored.body_html.as_deref(),
            *expected_html,
            "case {case:?}: unexpected stored HTML part, got {stored:?}"
        );
    }

    // A simple body with neither part names nothing to send.
    let refused = ses
        .send(json!({
            "FromEmailAddress": "solo@example.net",
            "Destination": {"ToAddresses": ["rcpt@example.org"]},
            "Content": {"Simple": {"Subject": {"Data": "empty"}, "Body": {}}},
        }))
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "BadRequestException"),
        "case no-body-part: expected 400 BadRequestException, got {} {:?}",
        refused.status,
        refused.body
    );
}

#[tokio::test]
async fn the_outbound_log_and_the_sending_quota_are_bounded() {
    let ses = Ses::new();
    assert_eq!(
        ses.create(json!({"EmailIdentity": "solo@example.net"}))
            .await
            .status,
        200,
        "expected the create to succeed"
    );
    ses.settle_verification("solo@example.net").await;

    // A body larger than the log retains is truncated, not stored whole.
    let oversized = "x".repeat(MAX_STORED_BODY_CHARS * 2 + 7);
    let reply = ses
        .send(json!({
            "FromEmailAddress": "solo@example.net",
            "Destination": {"ToAddresses": ["rcpt@example.org"]},
            "Content": {"Simple": {
                "Subject": {"Data": "big"},
                "Body": {"Text": {"Data": oversized}},
            }},
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "case oversized-body: expected 200, got {} {:?}",
        reply.status, reply.body
    );
    let stored_len = ses
        .sent_messages()
        .last()
        .and_then(|message| message.body.as_ref().map(String::len))
        .unwrap_or_default();
    assert_eq!(
        stored_len, MAX_STORED_BODY_CHARS,
        "case oversized-body: expected the retained body to be capped at {MAX_STORED_BODY_CHARS}, got {stored_len}"
    );

    // Messages outside the 24-hour window do not count against the quota, and the
    // log keeps only its newest entries.
    let long_ago = SystemTime::now()
        .checked_sub(SEND_QUOTA_WINDOW + Duration::from_secs(60))
        .expect("a timestamp a day ago");
    {
        let state = ses
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        for index in 0..MAX_SENT_MESSAGES {
            store.sent_messages.push(SentMessage {
                message_id: format!("expired-{index}"),
                from: Some("solo@example.net".to_string()),
                to: vec!["old@example.org".to_string()],
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
                sent_at: long_ago,
            });
        }
    }

    let reply = ses
        .send(json!({
            "FromEmailAddress": "solo@example.net",
            "Destination": {"ToAddresses": ["rcpt@example.org"]},
            "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}},
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "case expired-window: a message older than the quota window must not block a send, got {} {:?}",
        reply.status, reply.body
    );
    let log = ses.sent_messages();
    assert_eq!(
        log.len(),
        MAX_SENT_MESSAGES,
        "case log-cap: expected the log to retain {MAX_SENT_MESSAGES} entries, got {}",
        log.len()
    );
    assert!(
        log.iter().any(|message| message.message_id == reply.text("MessageId")),
        "case log-cap: the newest message must be retained"
    );
    assert!(
        !log.iter()
            .any(|message| message.message_id == "expired-0"),
        "case log-cap: the oldest entry must be evicted, got {:?}",
        log.first()
    );

    // The quota itself: accepting more recipients than the account's 24-hour quota
    // is refused, and the refusal adds nothing to the log.
    let quota = ses
        .engine
        .state::<ComAmazonawsSesv2SimpleemailserviceV2State>()
        .lock()
        .send_counters
        .max_24h;
    let mut charged = ses.sent_last_24h();
    assert!(
        charged < quota,
        "case quota: expected room in the quota before filling it, got {charged} of {quota}"
    );
    while charged < quota {
        let reply = ses
            .send(json!({
                "FromEmailAddress": "solo@example.net",
                "Destination": {"ToAddresses": ["rcpt@example.org"]},
                "Content": {"Simple": {"Subject": {"Data": "fill"}, "Body": {"Text": {"Data": "fill"}}}},
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case quota-fill: expected 200 while under the quota ({charged} of {quota}), got {} {:?}",
            reply.status, reply.body
        );
        charged = ses.sent_last_24h();
    }
    let before = ses.sent_messages().len();
    let refused = ses
        .send(json!({
            "FromEmailAddress": "solo@example.net",
            "Destination": {"ToAddresses": ["rcpt@example.org"]},
            "Content": {"Simple": {"Subject": {"Data": "over"}, "Body": {"Text": {"Data": "over"}}}},
        }))
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (429, "TooManyRequestsException"),
        "case quota-exceeded: expected 429 TooManyRequestsException once the quota is spent, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        ses.sent_messages().len(),
        before,
        "case quota-exceeded: a refused send must not be logged"
    );
}

#[tokio::test]
async fn create_refuses_more_identities_than_the_account_quota() {
    let ses = Ses::new();

    // Fill the table to SES's documented quota of 10,000 identities directly, so
    // the boundary is what the request meets.
    {
        let state = ses
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let now = SystemTime::now();
        for index in 0..MAX_IDENTITIES {
            let name = format!("filler-{index}.example.com");
            store.identities.insert(
                name.clone(),
                service_com_amazonaws_sesv2_simpleemailservice_v2::state::EmailIdentityRecord::new(
                    &name,
                    service_com_amazonaws_sesv2_simpleemailservice_v2::state::IdentityKind::Domain,
                    service_com_amazonaws_sesv2_simpleemailservice_v2::state::DkimState::not_started(),
                    now,
                ),
            );
        }
    }

    let reply = ses
        .create(json!({"EmailIdentity": "one-too-many.example.com"}))
        .await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "LimitExceededException"),
        "case identity-quota: expected 400 LimitExceededException at {MAX_IDENTITIES} identities, got {} {:?}",
        reply.status,
        reply.body
    );
}

#[tokio::test]
async fn send_refuses_what_it_cannot_accept() {
    let ses = Ses::new();
    assert_eq!(
        ses.create(json!({"EmailIdentity": "solo@example.net"}))
            .await
            .status,
        200,
        "expected the create to succeed"
    );
    ses.settle_verification("solo@example.net").await;

    let simple = json!({"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}});
    let destination = json!({"ToAddresses": ["rcpt@example.org"]});

    let cases: &[(&str, Value, u16, &str)] = &[
        (
            // `Content` is a required member: the request names no message.
            "no-content",
            json!({"FromEmailAddress": "solo@example.net", "Destination": destination}),
            400,
            "BadRequestException",
        ),
        (
            "content-with-no-form",
            json!({
                "FromEmailAddress": "solo@example.net",
                "Destination": destination,
                "Content": {},
            }),
            400,
            "BadRequestException",
        ),
        (
            "content-with-two-forms",
            json!({
                "FromEmailAddress": "solo@example.net",
                "Destination": destination,
                "Content": {
                    "Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}},
                    "Raw": {"Data": aws_smithy_types::base64::encode("From: solo@example.net\r\n\r\nbody")},
                },
            }),
            400,
            "BadRequestException",
        ),
        (
            "body-with-neither-text-nor-html",
            json!({
                "FromEmailAddress": "solo@example.net",
                "Destination": destination,
                "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {}}},
            }),
            400,
            "BadRequestException",
        ),
        (
            "no-sender",
            json!({"Destination": destination, "Content": simple}),
            400,
            "BadRequestException",
        ),
        (
            // A raw message may carry its sender in the MIME `From` header, which
            // this slice does not read: the request is valid, so it is answered as
            // unwritten rather than as the caller's mistake.
            "no-sender-with-raw-content",
            json!({
                "Destination": destination,
                "Content": {
                    "Raw": {"Data": aws_smithy_types::base64::encode("From: solo@example.net\r\n\r\nbody")},
                },
            }),
            501,
            "NotImplemented",
        ),
        (
            "no-recipient",
            json!({
                "FromEmailAddress": "solo@example.net",
                "Destination": {},
                "Content": simple,
            }),
            400,
            "BadRequestException",
        ),
        (
            // A raw message may address its recipients in the MIME document's own
            // headers, which this slice does not read: unwritten, not the caller's
            // mistake.
            "no-destination-with-raw-content",
            json!({
                "FromEmailAddress": "solo@example.net",
                "Content": {
                    "Raw": {"Data": aws_smithy_types::base64::encode("To: rcpt@example.org\r\n\r\nbody")},
                },
            }),
            501,
            "NotImplemented",
        ),
        (
            // "The address that you specify has to be verified."
            "unknown-sender",
            json!({
                "FromEmailAddress": "stranger@nowhere.example",
                "Destination": destination,
                "Content": simple,
            }),
            400,
            "MessageRejected",
        ),
        (
            // The domain of a verified ADDRESS identity does not authorise other
            // addresses in that domain.
            "sibling-of-an-address-identity",
            json!({
                "FromEmailAddress": "someone-else@example.net",
                "Destination": destination,
                "Content": simple,
            }),
            400,
            "MessageRejected",
        ),
    ];
    for (case, request, status, expected) in cases {
        let reply = ses.send(request.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (*status, *expected),
            "case {case:?}: expected {status} {expected}, got {} {:?} ({})",
            reply.status,
            reply.body,
            reply.message()
        );
    }

    // Nothing was logged and no quota was consumed by a refused request.
    assert!(
        ses.sent_messages().is_empty(),
        "a refused send must not be logged, got {:?}",
        ses.sent_messages()
    );
    assert_eq!(
        ses.sent_last_24h(),
        0.0,
        "a refused send must not consume the sending quota"
    );

    // Members that select a resource no implemented operation can create yet are
    // answered as unwritten, not accepted as if they had not been sent.
    // `ListManagementOptions` is NOT one of them: it names a contact list, which this
    // slice holds, so it is resolved against the contact-list table (see
    // `send_list_management_options`).
    for member in [
        json!({"TenantName": "tenant-a"}),
        json!({"EndpointId": "endpoint-a"}),
    ] {
        let mut request = json!({
            "FromEmailAddress": "solo@example.net",
            "Destination": destination,
            "Content": simple,
        });
        for (key, value) in member.as_object().expect("an object") {
            request[key] = value.clone();
        }
        let reply = ses.send(request).await;
        assert_eq!(
            reply.status, 501,
            "case {member:?}: expected 501, got {} {:?}",
            reply.status, reply.body
        );
    }

    // A templated message needs the stored template and renderer that
    // CreateEmailTemplate owns, so it is answered as unwritten too.
    let reply = ses
        .send(json!({
            "FromEmailAddress": "solo@example.net",
            "Destination": destination,
            "Content": {"Template": {"TemplateName": "welcome", "TemplateData": "{}"}},
        }))
        .await;
    assert_eq!(
        reply.status, 501,
        "case templated-content: expected 501, got {} {:?}",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn the_domain_of_an_identity_matches_without_regard_to_case() {
    // DNS compares domain labels without regard to case, so one domain identity is
    // one identity however a request spells its domain; the local part of an address
    // belongs to the receiving mailbox, so it is compared as supplied. Each row
    // spells the SAME identity differently.
    let domain_spellings = ["Example.COM", "example.com", "EXAMPLE.COM", "eXaMpLe.cOm"];

    for spelling in domain_spellings {
        let ses = Ses::new();
        assert_eq!(
            ses.create(json!({"EmailIdentity": "Example.COM"})).await.status,
            200,
            "case {spelling:?}: expected the create to succeed"
        );

        // A second create of the same domain, spelled differently, is the same
        // resource.
        let again = ses.create(json!({"EmailIdentity": spelling})).await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (400, "AlreadyExistsException"),
            "case {spelling:?}: expected 400 AlreadyExistsException for the same domain, got {} {:?}",
            again.status,
            again.body
        );

        // Every reader finds it under any spelling, and reports the value it was
        // created with.
        let read = ses.get(spelling).await;
        assert_eq!(
            read.status, 200,
            "case {spelling:?}: expected GetEmailIdentity to find the domain, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("IdentityType"),
            "DOMAIN",
            "case {spelling:?}: expected IdentityType DOMAIN, got {:?}",
            read.body
        );
        let listed = ses.list(json!({})).await;
        assert_eq!(
            Ses::listed(&listed),
            vec!["Example.COM".to_string()],
            "case {spelling:?}: the listing must report the value the identity was created with, got {:?}",
            listed.body
        );

        // A verified domain identity authorises an address in that domain whatever
        // case the request spells the domain in.
        ses.settle_verification(spelling).await;
        let sent = ses
            .send(json!({
                "FromEmailAddress": format!("sender@{spelling}"),
                "Destination": {"ToAddresses": ["rcpt@example.org"]},
                "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}},
            }))
            .await;
        assert_eq!(
            sent.status, 200,
            "case {spelling:?}: the domain identity must authorise sender@{spelling}, got {} {:?}",
            sent.status, sent.body
        );

        // And a delete under any spelling removes the one identity.
        assert_eq!(
            ses.delete(spelling).await.status,
            200,
            "case {spelling:?}: expected the delete to succeed"
        );
        assert_eq!(
            ses.get("Example.COM").await.status,
            404,
            "case {spelling:?}: the identity must be gone after the delete"
        );
    }

    // The local part is NOT case-folded: two addresses that differ only in their
    // local part are two identities.
    let ses = Ses::new();
    assert_eq!(
        ses.create(json!({"EmailIdentity": "Case@Example.net"}))
            .await
            .status,
        200,
        "case local-part: expected the create to succeed"
    );
    assert_eq!(
        ses.get("Case@EXAMPLE.net").await.status,
        200,
        "case local-part: the domain part of an address is case-insensitive"
    );
    assert_eq!(
        ses.get("case@example.net").await.status,
        404,
        "case local-part: a different local part is a different identity"
    );
}

#[tokio::test]
async fn the_sender_may_be_spelled_as_a_display_name_address() {
    // RFC 5322 lets an address member carry a display name around the addr-spec, and
    // the mailbox it names is the same one: the identity that authorises the send is
    // found by the addr-spec, whatever the display name says.
    let ses = Ses::new();
    assert_eq!(
        ses.create(json!({"EmailIdentity": "solo@example.net"}))
            .await
            .status,
        200,
        "expected the create to succeed"
    );
    ses.settle_verification("solo@example.net").await;

    let spellings = [
        "solo@example.net",
        "<solo@example.net>",
        "Solo Sender <solo@example.net>",
        "\"Solo, Sender\" <solo@example.net>",
        "Solo Sender <solo@EXAMPLE.net>",
    ];
    for spelling in spellings {
        let reply = ses
            .send(json!({
                "FromEmailAddress": spelling,
                "Destination": {"ToAddresses": ["rcpt@example.org"]},
                "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}},
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {spelling:?}: the verified mailbox must authorise this spelling, got {} {:?}",
            reply.status, reply.body
        );
        // The log keeps the From the request carried, not a rewritten one.
        let message_id = reply.text("MessageId");
        let log = ses.sent_messages();
        let stored = log
            .iter()
            .find(|message| message.message_id == message_id)
            .unwrap_or_else(|| panic!("case {spelling:?}: the message was not persisted"));
        assert_eq!(
            stored.from.as_deref(),
            Some(spelling),
            "case {spelling:?}: expected the stored sender as supplied, got {stored:?}"
        );
    }

    // A display name around an address nobody verified is still refused.
    let refused = ses
        .send(json!({
            "FromEmailAddress": "Solo Sender <stranger@nowhere.example>",
            "Destination": {"ToAddresses": ["rcpt@example.org"]},
            "Content": {"Simple": {"Subject": {"Data": "hi"}, "Body": {"Text": {"Data": "hello"}}}},
        }))
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "MessageRejected"),
        "case unverified-display-name: expected 400 MessageRejected, got {} {:?}",
        refused.status,
        refused.body
    );
}
