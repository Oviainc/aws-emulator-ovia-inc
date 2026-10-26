// SPDX-License-Identifier: CC0-1.0

//! The S/MIME certificate-association surface of an email identity, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reads the engine's own state where a response cannot show what was
//! persisted.
//!
//! The rules these cases pin:
//!
//! * an association belongs to ONE address — the identity's own when the identity is an
//!   address, the `FromAddress` within the domain when it is a domain — and a listing of
//!   the identity reports every association it holds, each with the address it applies
//!   to, the certificate it names and the status it currently carries;
//! * a created association starts out being prepared for signing and becomes ready for it
//!   when the clock says so; a removed one stops signing at once and is cleaned up
//!   afterwards;
//! * a second association for an address that already has one is refused, and so is a
//!   `FromAddress` that does not name an address the identity covers;
//! * an identity that still signs with a certificate cannot be deleted;
//! * a paginated listing answers with at most `PageSize` associations and a `NextToken`
//!   only while associations remain behind that page, so a walk that follows the token
//!   terminates and visits every association exactly once.
//!
//! The tables below span what those rules generalise over: the kind of identity (address,
//! domain), the form of `FromAddress` (absent, the identity itself, a differently cased
//! spelling of it, an address in the domain, an address in a subdomain), the status an
//! association is in, the number of stored associations and the page size.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::IDENTITY_CERTIFICATE_STATUS_VALUES;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    identity_arn, identity_key, ComAmazonawsSesv2SimpleemailserviceV2State, IdentityCertificateRecord,
    CERTIFICATE_SETTLE, MAX_CERTIFICATES_PER_IDENTITY,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A well-formed ACM certificate ARN, as `CertificateArn`'s modelled pattern requires.
/// The id part only has to be an identifier the pattern accepts; which certificate it
/// names is ACM's business, not this service's.
fn certificate_arn(id: &str) -> String {
    format!("arn:aws:acm:us-east-1:123456789012:certificate/{id}")
}

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
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

    /// The `Certificates` a listing reported, in the order it reported them.
    fn certificates(&self) -> Vec<Value> {
        self.body
            .get("Certificates")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `NextToken` a page handed back, if any.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// A string member of one listed certificate, or `""` when absent.
fn member(certificate: &Value, name: &str) -> String {
    certificate
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The emulated service behind its router, plus the shared engine so a test can read
/// what a handler persisted and move the instant a clock-owed transition falls due.
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
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
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

    async fn get_identity(&self, identity: &str) -> Reply {
        self.call("GET", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn delete_identity(&self, identity: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/identities/{identity}"), None)
            .await
    }

    async fn associate(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identity/certificates", Some(body))
            .await
    }

    async fn disassociate(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identity/certificates/delete", Some(body))
            .await
    }

    async fn list_certificates(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/identity/certificates/list", Some(body))
            .await
    }

    /// Move the instant at which the association for `from_address` has its next
    /// transition fall due.
    ///
    /// The handler stores a due instant rather than sleeping, so a test drives the
    /// transition by moving that instant instead of waiting on the wall clock — which is
    /// what keeps every assertion about a transient status independent of how loaded the
    /// machine is.
    fn set_certificate_due(&self, identity: &str, from_address: &str, due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        let certificate = record
            .certificates
            .get_mut(&identity_key(from_address))
            .unwrap_or_else(|| {
                panic!("case {from_address:?}: the association must exist in the store")
            });
        certificate.status_due_at = due_at;
    }

    /// Hold an association's next clock-owed transition off, so an assertion about the
    /// status it carries now cannot race the span.
    fn hold_certificate(&self, identity: &str, from_address: &str) {
        self.set_certificate_due(
            identity,
            from_address,
            SystemTime::now() + CERTIFICATE_SETTLE + Duration::from_secs(3600),
        );
    }

    /// Bring an association's next clock-owed transition into the past, so the next read
    /// materialises it.
    fn due_certificate(&self, identity: &str, from_address: &str) {
        self.set_certificate_due(
            identity,
            from_address,
            SystemTime::now()
                .checked_sub(Duration::from_millis(1))
                .expect("an instant just before now"),
        );
    }

    /// The span the handler stored between accepting a call and the transition it owes.
    fn certificate_span(&self, identity: &str, from_address: &str, from: SystemTime) -> Duration {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        let record = store
            .identities
            .get(&identity_key(identity))
            .unwrap_or_else(|| panic!("case {identity:?}: the identity must exist in the store"));
        let certificate = record
            .certificates
            .get(&identity_key(from_address))
            .unwrap_or_else(|| {
                panic!("case {from_address:?}: the association must exist in the store")
            });
        certificate
            .status_due_at
            .duration_since(from)
            .expect("the due instant is at or after the call that started the transition")
    }

    /// Create `identity`, associate `arn` with `from_address` on it, and hold the
    /// association's `PROVISIONING` -> `ACTIVE` transition off.
    async fn with_held_association(&self, identity: &str, from_address: &str, arn: &str) {
        let created = self.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {identity:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );
        let associated = self
            .associate(json!({
                "EmailIdentity": identity,
                "FromAddress": from_address,
                "CertificateArn": arn,
            }))
            .await;
        assert_eq!(
            associated.status, 200,
            "case {identity:?}/{from_address:?}: expected the association to be accepted, got {} {:?}",
            associated.status, associated.body
        );
        self.hold_certificate(identity, from_address);
    }
}

/// One row of the association table: the identity the certificate is associated with, the
/// `FromAddress` the request carries (if any), and the address the listing must then
/// report the association under.
struct AssociationCase {
    case: &'static str,
    identity: &'static str,
    from_address: Option<&'static str>,
    reported_from_address: &'static str,
}

/// The rule spans the kind of identity and the form `FromAddress` arrives in, so the rows
/// cross an address identity with no `FromAddress`, with the identity repeated exactly and
/// with the same mailbox spelled with a differently cased domain, and a domain identity
/// with an address in the domain and one in a subdomain of it.
const ASSOCIATION_CASES: &[AssociationCase] = &[
    AssociationCase {
        case: "address identity, no FromAddress",
        identity: "signer@example.com",
        from_address: None,
        reported_from_address: "signer@example.com",
    },
    AssociationCase {
        case: "address identity, FromAddress repeating the identity",
        identity: "repeat@example.com",
        from_address: Some("repeat@example.com"),
        reported_from_address: "repeat@example.com",
    },
    AssociationCase {
        case: "address identity, FromAddress with a differently cased domain",
        identity: "cased@example.com",
        from_address: Some("cased@EXAMPLE.com"),
        reported_from_address: "cased@EXAMPLE.com",
    },
    AssociationCase {
        case: "domain identity, FromAddress in the domain",
        identity: "example.com",
        from_address: Some("sender@example.com"),
        reported_from_address: "sender@example.com",
    },
    AssociationCase {
        case: "domain identity, FromAddress in a subdomain",
        identity: "example.com",
        from_address: Some("sender@mail.corp.example.com"),
        reported_from_address: "sender@mail.corp.example.com",
    },
];

#[tokio::test]
async fn an_association_round_trips_through_the_identity_listing() {
    for case in ASSOCIATION_CASES {
        let AssociationCase {
            case,
            identity,
            from_address,
            reported_from_address,
        } = case;
        let ses = Ses::new();
        let created = ses.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );

        // Nothing is associated with a fresh identity.
        let empty = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            empty.status, 200,
            "case {case:?}: expected a listing of a fresh identity to be served, got {} {:?}",
            empty.status, empty.body
        );
        assert!(
            empty.certificates().is_empty(),
            "case {case:?}: expected no certificates on a fresh identity, got {:?}",
            empty.body
        );

        let arn = certificate_arn("11111111-1111-1111-1111-111111111111");
        let mut request = json!({ "EmailIdentity": identity, "CertificateArn": arn });
        if let Some(address) = from_address {
            request["FromAddress"] = json!(address);
        }
        let before = SystemTime::now();
        let associated = ses.associate(request).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: expected the association to be accepted, got {} {:?}",
            associated.status, associated.body
        );

        // The transition is owed at an instant the handler stored, so hold it off before
        // asserting the status the caller sees straight after the call.
        let span = ses.certificate_span(identity, reported_from_address, before);
        assert!(
            span > Duration::ZERO,
            "case {case:?}: expected a non-zero span before the association becomes ACTIVE, got {span:?}"
        );
        ses.hold_certificate(identity, reported_from_address);

        let listed = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        let certificates = listed.certificates();
        assert_eq!(
            certificates.len(),
            1,
            "case {case:?}: expected exactly one association, got {:?}",
            listed.body
        );
        let certificate = &certificates[0];
        assert_eq!(
            member(certificate, "FromAddress"),
            *reported_from_address,
            "case {case:?}: expected the association to apply to {reported_from_address:?}, got {certificate:?}"
        );
        assert_eq!(
            member(certificate, "CertificateArn"),
            arn,
            "case {case:?}: expected the certificate ARN the request named, got {certificate:?}"
        );
        assert_eq!(
            member(certificate, "Status"),
            "PROVISIONING",
            "case {case:?}: expected a fresh association to be PROVISIONING, got {certificate:?}"
        );
        assert!(
            IDENTITY_CERTIFICATE_STATUS_VALUES.contains(&member(certificate, "Status").as_str()),
            "case {case:?}: expected a modelled IdentityCertificateStatus, got {certificate:?}"
        );
        assert!(
            listed.next_token().is_none(),
            "case {case:?}: expected no NextToken for a single-page listing, got {:?}",
            listed.body
        );

        // The transition the clock owes: once due, a read reports the association ready
        // for signing.
        ses.due_certificate(identity, reported_from_address);
        let active = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&active.certificates()[0], "Status"),
            "ACTIVE",
            "case {case:?}: expected the association to be ACTIVE once its transition fell due, got {:?}",
            active.body
        );

        // Removing it stops the signing at once, and the association is then cleaned up.
        let removed_request = match from_address {
            Some(address) => json!({ "EmailIdentity": identity, "FromAddress": address }),
            None => json!({ "EmailIdentity": identity }),
        };
        let removed = ses.disassociate(removed_request.clone()).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected the association to be removed, got {} {:?}",
            removed.status, removed.body
        );
        ses.hold_certificate(identity, reported_from_address);
        let inactive = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&inactive.certificates()[0], "Status"),
            "INACTIVE",
            "case {case:?}: expected a removed association to be INACTIVE, got {:?}",
            inactive.body
        );

        // The clean-up: INACTIVE falls to DEPROVISIONING, and then the association is
        // gone, which frees the address to take a new certificate.
        ses.due_certificate(identity, reported_from_address);
        let deprovisioning = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&deprovisioning.certificates()[0], "Status"),
            "DEPROVISIONING",
            "case {case:?}: expected the clean-up to report DEPROVISIONING, got {:?}",
            deprovisioning.body
        );
        ses.due_certificate(identity, reported_from_address);
        let gone = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert!(
            gone.certificates().is_empty(),
            "case {case:?}: expected the cleaned-up association to be gone, got {:?}",
            gone.body
        );

        // Idempotent: removing what is no longer there changes nothing and is served.
        let again = ses.disassociate(removed_request).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a removal with no matching association to succeed, got {} {:?}",
            again.status, again.body
        );
    }
}

/// One row of the refusal table: the request, and the status, error code and message the
/// service must answer it with.
struct RefusalCase {
    case: &'static str,
    identity: &'static str,
    from_address: Option<&'static str>,
    status: u16,
    code: &'static str,
    message: String,
}

/// The refusals an association is subject to: an identity this account does not hold, a
/// domain identity with no address to apply the certificate to, an address outside the
/// identity (including a domain whose name merely ends with the identity's, which DNS
/// does not make a subdomain), and an address identity the `FromAddress` does not name.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "identity this account does not hold",
            identity: "absent.example.com",
            from_address: Some("sender@absent.example.com"),
            status: 404,
            code: "NotFoundException",
            message: "Email identity <absent.example.com> does not exist.".to_string(),
        },
        RefusalCase {
            case: "domain identity with no FromAddress",
            identity: "example.com",
            from_address: None,
            status: 400,
            code: "BadRequestException",
            message: "FromAddress is required when EmailIdentity is a domain.".to_string(),
        },
        RefusalCase {
            case: "FromAddress in another domain",
            identity: "example.com",
            from_address: Some("sender@other.example.org"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <sender@other.example.org> does not belong to the domain \
                      <example.com> or one of its subdomains."
                .to_string(),
        },
        RefusalCase {
            case: "FromAddress in a domain that merely ends with the identity",
            identity: "example.com",
            from_address: Some("sender@notexample.com"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <sender@notexample.com> does not belong to the domain \
                      <example.com> or one of its subdomains."
                .to_string(),
        },
        RefusalCase {
            case: "FromAddress that is not an address at all",
            identity: "example.com",
            from_address: Some("example.com"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <example.com> does not belong to the domain <example.com> \
                      or one of its subdomains."
                .to_string(),
        },
        RefusalCase {
            case: "address identity with a FromAddress naming another mailbox",
            identity: "signer@example.com",
            from_address: Some("other@example.com"),
            status: 400,
            code: "BadRequestException",
            message: "FromAddress <other@example.com> must exactly match the email identity \
                      <signer@example.com>."
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn an_association_the_identity_cannot_take_is_refused() {
    let arn = certificate_arn("22222222-2222-2222-2222-222222222222");
    for case in refusal_cases() {
        let ses = Ses::new();
        // Every case but the unknown-identity one needs the identity to exist, so the
        // refusal under test is the one the case is about.
        for identity in ["example.com", "signer@example.com"] {
            let created = ses.create_identity(identity).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected {identity:?} to be created, got {} {:?}",
                case.case, created.status, created.body
            );
        }

        let mut request = json!({ "EmailIdentity": case.identity, "CertificateArn": arn });
        if let Some(address) = case.from_address {
            request["FromAddress"] = json!(address);
        }
        let refused = ses.associate(request).await;
        assert_eq!(
            refused.status, case.status,
            "case {:?}: expected HTTP {}, got {} {:?}",
            case.case, case.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            case.code,
            "case {:?}: expected {}, got {:?} ({:?})",
            case.case,
            case.code,
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            case.message,
            "case {:?}: expected the refusal message {:?}, got {:?}",
            case.case,
            case.message,
            refused.message()
        );
    }
}

#[tokio::test]
async fn one_address_holds_one_certificate_at_a_time() {
    // "Each email address can have only one certificate association": the second call is
    // refused whichever status the first association is in, until it has been removed and
    // cleaned up.
    let holding_statuses = ["PROVISIONING", "ACTIVE", "INACTIVE", "DEPROVISIONING"];
    for status in holding_statuses {
        let ses = Ses::new();
        let identity = "example.com";
        let from_address = "sender@example.com";
        ses.with_held_association(
            identity,
            from_address,
            &certificate_arn("33333333-3333-3333-3333-333333333333"),
        )
        .await;
        // Walk the association into the status this row is about.
        match status {
            "PROVISIONING" => {}
            "ACTIVE" => {
                ses.due_certificate(identity, from_address);
                ses.list_certificates(json!({ "EmailIdentity": identity }))
                    .await;
                ses.hold_certificate(identity, from_address);
            }
            _ => {
                ses.due_certificate(identity, from_address);
                ses.list_certificates(json!({ "EmailIdentity": identity }))
                    .await;
                ses.disassociate(json!({
                    "EmailIdentity": identity,
                    "FromAddress": from_address,
                }))
                .await;
                if status == "DEPROVISIONING" {
                    ses.due_certificate(identity, from_address);
                    ses.list_certificates(json!({ "EmailIdentity": identity }))
                        .await;
                }
                ses.hold_certificate(identity, from_address);
            }
        }

        let listed = ses
            .list_certificates(json!({ "EmailIdentity": identity }))
            .await;
        assert_eq!(
            member(&listed.certificates()[0], "Status"),
            status,
            "case {status:?}: expected the association to be in this status before the second \
             call, got {:?}",
            listed.body
        );

        let second = ses
            .associate(json!({
                "EmailIdentity": identity,
                "FromAddress": from_address,
                "CertificateArn": certificate_arn("44444444-4444-4444-4444-444444444444"),
            }))
            .await;
        if status == "DEPROVISIONING" {
            // "...unless the existing association is in the DEPROVISIONING state": one
            // being cleaned up is on its way out, so the address may take a new one.
            assert_eq!(
                second.status, 200,
                "case {status:?}: expected a new association over one being cleaned up, got {} {:?}",
                second.status, second.body
            );
            ses.hold_certificate(identity, from_address);
            let replaced = ses
                .list_certificates(json!({ "EmailIdentity": identity }))
                .await;
            assert_eq!(
                member(&replaced.certificates()[0], "CertificateArn"),
                certificate_arn("44444444-4444-4444-4444-444444444444"),
                "case {status:?}: expected the new certificate to be the one reported, got {:?}",
                replaced.body
            );
            assert_eq!(
                member(&replaced.certificates()[0], "Status"),
                "PROVISIONING",
                "case {status:?}: expected the replacement to start out PROVISIONING, got {:?}",
                replaced.body
            );
            continue;
        }
        assert_eq!(
            second.status, 400,
            "case {status:?}: expected the duplicate association to be refused, got {} {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.error_code(),
            "AlreadyExistsException",
            "case {status:?}: expected AlreadyExistsException, got {:?} ({:?})",
            second.error_code(),
            second.message()
        );
        assert_eq!(
            second.message(),
            format!(
                "A certificate is already associated with sender <{from_address}> on identity \
                 <{identity}>."
            ),
            "case {status:?}: expected the refusal to name the sender and the identity, got {:?}",
            second.message()
        );
    }
}

#[tokio::test]
async fn an_identity_that_still_signs_with_a_certificate_cannot_be_deleted() {
    let ses = Ses::new();
    let identity = "example.com";
    let from_address = "sender@example.com";
    ses.with_held_association(
        identity,
        from_address,
        &certificate_arn("55555555-5555-5555-5555-555555555555"),
    )
    .await;

    // Both statuses an association signs with hold the identity: one being prepared for
    // signing and one ready for it.
    for status in ["PROVISIONING", "ACTIVE"] {
        if status == "ACTIVE" {
            ses.due_certificate(identity, from_address);
            ses.list_certificates(json!({ "EmailIdentity": identity }))
                .await;
            ses.hold_certificate(identity, from_address);
        }
        let refused = ses.delete_identity(identity).await;
        assert_eq!(
            refused.status, 400,
            "case {status:?}: expected the delete to be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {status:?}: expected BadRequestException, got {:?} ({:?})",
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            format!(
                "Cannot delete <{}> because it has certificates associated with it. \
                 Disassociate all certificates and try again.",
                identity_arn("us-east-1", identity)
            ),
            "case {status:?}: expected the refusal to name the identity by ARN, got {:?}",
            refused.message()
        );
        let still_there = ses.get_identity(identity).await;
        assert_eq!(
            still_there.status, 200,
            "case {status:?}: expected the refused delete to leave the identity, got {} {:?}",
            still_there.status, still_there.body
        );
    }

    // Once nothing signs for it, the identity goes.
    let removed = ses
        .disassociate(json!({
            "EmailIdentity": identity,
            "FromAddress": from_address,
        }))
        .await;
    assert_eq!(
        removed.status, 200,
        "expected the association to be removed, got {} {:?}",
        removed.status, removed.body
    );
    let deleted = ses.delete_identity(identity).await;
    assert_eq!(
        deleted.status, 200,
        "expected the delete to be served once nothing signs for the identity, got {} {:?}",
        deleted.status, deleted.body
    );
    let read_back = ses.get_identity(identity).await;
    assert_eq!(
        read_back.status, 404,
        "expected the deleted identity to be gone, got {} {:?}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.message(),
        "Email identity <example.com> does not exist.",
        "expected the deleted identity's own not-found sentence, got {:?}",
        read_back.message()
    );
}

#[tokio::test]
async fn an_identity_holds_at_most_the_bounded_number_of_associations() {
    let ses = Ses::new();
    let identity = "example.com";
    let created = ses.create_identity(identity).await;
    assert_eq!(
        created.status, 200,
        "expected the identity to be created, got {} {:?}",
        created.status, created.body
    );

    // Fill the identity to its bound through the store rather than through the wire, so
    // the case runs in one request instead of thousands; the refusal under test is still
    // the handler's.
    {
        let state = ses
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .identities
            .get_mut(&identity_key(identity))
            .expect("the identity must exist in the store");
        let now = SystemTime::now();
        for index in 0..MAX_CERTIFICATES_PER_IDENTITY {
            let address = format!("bulk{index}@example.com");
            record.certificates.insert(
                identity_key(&address),
                IdentityCertificateRecord::new(
                    &address,
                    &certificate_arn(&format!("7777777{index}")),
                    now,
                ),
            );
        }
    }

    let past_the_bound = ses
        .associate(json!({
            "EmailIdentity": identity,
            "FromAddress": "one-too-many@example.com",
            "CertificateArn": certificate_arn("88888888-8888-8888-8888-888888888888"),
        }))
        .await;
    assert_eq!(
        past_the_bound.status, 400,
        "expected the association past the bound to be refused, got {} {:?}",
        past_the_bound.status, past_the_bound.body
    );
    assert_eq!(
        past_the_bound.error_code(),
        "BadRequestException",
        "expected BadRequestException, got {:?} ({:?})",
        past_the_bound.error_code(),
        past_the_bound.message()
    );
    assert_eq!(
        past_the_bound.message(),
        format!(
            "Email identity <{identity}> already holds the maximum of \
             {MAX_CERTIFICATES_PER_IDENTITY} certificate associations."
        ),
        "expected the refusal to name the bound it is past, got {:?}",
        past_the_bound.message()
    );

    // An association an address already holds is still refused as a duplicate at the
    // bound, rather than as a growth the store cannot take.
    let duplicate = ses
        .associate(json!({
            "EmailIdentity": identity,
            "FromAddress": "bulk0@example.com",
            "CertificateArn": certificate_arn("99999999-9999-9999-9999-999999999999"),
        }))
        .await;
    assert_eq!(
        duplicate.error_code(),
        "AlreadyExistsException",
        "expected the duplicate at the bound to be refused as one, got {:?} ({:?})",
        duplicate.error_code(),
        duplicate.message()
    );
}

/// One row of the pagination table: how many associations the identity holds and the
/// `PageSize` the walk asks for.
struct PageCase {
    case: &'static str,
    stored: usize,
    page_size: Option<usize>,
}

/// The pagination rule spans the association count and the page size independently, so
/// the rows cross an empty, a single and a multi-association identity with an absent page
/// size, a page size of one, one that divides the count, one that does not, and one larger
/// than the count.
const PAGE_CASES: &[PageCase] = &[
    PageCase {
        case: "nothing associated, no page size",
        stored: 0,
        page_size: None,
    },
    PageCase {
        case: "nothing associated, page size 1",
        stored: 0,
        page_size: Some(1),
    },
    PageCase {
        case: "one association, page size 1",
        stored: 1,
        page_size: Some(1),
    },
    PageCase {
        case: "one association, no page size",
        stored: 1,
        page_size: None,
    },
    PageCase {
        case: "two associations, page size 1",
        stored: 2,
        page_size: Some(1),
    },
    PageCase {
        case: "ten associations, page size 1",
        stored: 10,
        page_size: Some(1),
    },
    PageCase {
        case: "ten associations, page size 5",
        stored: 10,
        page_size: Some(5),
    },
    PageCase {
        case: "ten associations, page size 4",
        stored: 10,
        page_size: Some(4),
    },
    PageCase {
        case: "ten associations, page size 100",
        stored: 10,
        page_size: Some(100),
    },
    PageCase {
        case: "ten associations, no page size",
        stored: 10,
        page_size: None,
    },
];

#[tokio::test]
async fn a_certificate_walk_visits_every_association_exactly_once() {
    for case in PAGE_CASES {
        let PageCase {
            case,
            stored,
            page_size,
        } = case;
        let ses = Ses::new();
        let identity = "example.com";
        let created = ses.create_identity(identity).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );

        let expected: BTreeSet<String> = (0..*stored)
            .map(|index| format!("sender{index}@example.com"))
            .collect();
        for (index, address) in expected.iter().enumerate() {
            let associated = ses
                .associate(json!({
                    "EmailIdentity": identity,
                    "FromAddress": address,
                    "CertificateArn": certificate_arn(&format!("6666666{index}")),
                }))
                .await;
            assert_eq!(
                associated.status, 200,
                "case {case:?}: expected {address:?} to be associated, got {} {:?}",
                associated.status, associated.body
            );
        }

        // One request per association plus one for the final page is the most any page
        // size can need; a request past that means the token loop does not terminate.
        let budget = stored + 2;
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut requests = 0;
        loop {
            requests += 1;
            assert!(
                requests <= budget,
                "case {case:?}: expected the walk to terminate within {budget} requests, \
                 visited {seen:?}"
            );
            let mut request = json!({ "EmailIdentity": identity });
            if let Some(size) = page_size {
                request["PageSize"] = json!(size);
            }
            if let Some(value) = &token {
                request["NextToken"] = json!(value);
            }
            let page = ses.list_certificates(request).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected the page to be served, got {} {:?}",
                page.status, page.body
            );
            let certificates = page.certificates();
            if let Some(size) = page_size {
                assert!(
                    certificates.len() <= *size,
                    "case {case:?}: expected at most {size} associations in a page, got {:?}",
                    page.body
                );
            }
            seen.extend(certificates.iter().map(|row| member(row, "FromAddress")));
            match page.next_token() {
                None => break,
                Some(next) => {
                    assert!(
                        !certificates.is_empty(),
                        "case {case:?}: expected a page handing back a token to carry \
                         associations, got {:?}",
                        page.body
                    );
                    token = Some(next);
                }
            }
        }

        assert_eq!(
            seen.len(),
            *stored,
            "case {case:?}: expected the walk to visit {stored} associations, visited {seen:?}"
        );
        let visited: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            visited, expected,
            "case {case:?}: expected the walk to visit every stored association exactly once, \
             visited {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: expected the walk to enumerate associations in a stable order, \
             visited {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_refuses_a_token_it_did_not_issue() {
    let ses = Ses::new();
    let identity = "example.com";
    ses.create_identity(identity).await;

    // A token from somewhere else is not a position in this listing: a base64 value of
    // another listing's making, and a value that is not base64 at all.
    let foreign = ::aws_smithy_types::base64::encode("sesv2:contacts:someone@example.com");
    for token in [foreign.as_str(), "not-a-token"] {
        let refused = ses
            .list_certificates(json!({ "EmailIdentity": identity, "NextToken": token }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {token:?}: expected the foreign token to be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {token:?}: expected BadRequestException, got {:?} ({:?})",
            refused.error_code(),
            refused.message()
        );
        assert_eq!(
            refused.message(),
            "NextToken is not a token returned by ListEmailIdentityCertificates.",
            "case {token:?}: expected the token refusal's own sentence, got {:?}",
            refused.message()
        );
    }

    // A listing and a removal of an identity nobody created name the identity they could
    // not find.
    for reply in [
        ses.list_certificates(json!({ "EmailIdentity": "absent.example.com" }))
            .await,
        ses.disassociate(json!({
            "EmailIdentity": "absent.example.com",
            "FromAddress": "sender@absent.example.com",
        }))
        .await,
    ] {
        assert_eq!(
            reply.status, 404,
            "expected an unknown identity to be reported as not found, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.message(),
            "Email identity <absent.example.com> does not exist.",
            "expected the identity's own not-found sentence, got {:?}",
            reply.message()
        );
    }

    // A removal needs the address when the identity is a domain, as the association is
    // the domain's one for that address.
    let no_address = ses.disassociate(json!({ "EmailIdentity": identity })).await;
    assert_eq!(
        no_address.status, 400,
        "expected a removal with no FromAddress on a domain to be refused, got {} {:?}",
        no_address.status, no_address.body
    );
    assert_eq!(
        no_address.message(),
        "FromAddress is required when EmailIdentity is a domain.",
        "expected the missing-FromAddress sentence, got {:?}",
        no_address.message()
    );
}
