// SPDX-License-Identifier: CC0-1.0

//! The `NotFoundException` a missing email identity is refused with, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label decoding, the handler and error serialization
//! are all exercised.
//!
//! The rule these cases pin: an operation that resolves an email identity by name and
//! does not find it is refused 404 `NotFoundException` with the sentence SES words for
//! THAT operation — `Email identity <NAME> does not exist.` on the identity reads and the
//! certificate operations, which quote the name in angle brackets, and `Email identity
//! NAME does not exist.` on `DeleteEmailIdentity` and the writes that hang an attribute
//! or a policy off an identity, which do not. The only variable part of either sentence is
//! the `EmailIdentity` the request spelled, so the two forms differ in exactly the
//! brackets and nothing else.
//!
//! The tables below span what the rule generalises over: the kind and the spelling of the
//! requested identity (an address, a domain, a subdomain, a mixed-case domain and an
//! address whose local part carries a tag), and which operation resolved it — over all
//! four HTTP methods the surface uses, with the name in the path and with the name in the
//! body.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
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
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, \
                 SignedHeaders=host, Signature=00",
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
}

/// One identity the account does not hold, and an address the certificate operations can
/// name for it (they require one when the identity is a domain).
struct Case {
    identity: &'static str,
    from_address: &'static str,
}

/// The forms an `Identity` takes: an email address, a domain, a subdomain, a domain in
/// mixed case (DNS compares labels without regard to case, so this names no identity
/// either) and an address whose local part carries a tag.
const CASES: &[Case] = &[
    Case {
        identity: "absent@example.com",
        from_address: "absent@example.com",
    },
    Case {
        identity: "absent.example.com",
        from_address: "sender@absent.example.com",
    },
    Case {
        identity: "mail.absent.example.net",
        from_address: "sender@mail.absent.example.net",
    },
    Case {
        identity: "Absent.Example.ORG",
        from_address: "sender@Absent.Example.ORG",
    },
    Case {
        identity: "absent+tag@example.net",
        from_address: "absent+tag@example.net",
    },
];

/// A sending-authorization policy document, so the policy create reaches the identity
/// lookup rather than being refused for its document.
fn policy_document() -> String {
    "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Action\":\"ses:SendEmail\",\
     \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}]}"
        .to_string()
}

/// The sentence the operations that QUOTE the identity answer.
fn quoted(identity: &str) -> String {
    format!("Email identity <{identity}> does not exist.")
}

/// The sentence the operations that name the identity UNQUOTED answer.
fn unquoted(identity: &str) -> String {
    format!("Email identity {identity} does not exist.")
}

/// Every implemented operation that resolves an email identity and quotes the name it did
/// not find, each answered once for `case`.
async fn quoting_refusals(ses: &Ses, case: &Case) -> Vec<(&'static str, Reply)> {
    let identity = case.identity;
    vec![
        ("GetEmailIdentity", ses.get_identity(identity).await),
        (
            "GetEmailIdentityPolicies",
            ses.call(
                "GET",
                &format!("/v2/email/identities/{identity}/policies"),
                None,
            )
            .await,
        ),
        (
            // The name arrives in the BODY here, so the wording cannot be coming from
            // the request line.
            "ListEmailIdentityCertificates",
            ses.call(
                "POST",
                "/v2/email/identity/certificates/list",
                Some(json!({ "EmailIdentity": identity })),
            )
            .await,
        ),
        (
            "AssociateEmailIdentityCertificate",
            ses.call(
                "POST",
                "/v2/email/identity/certificates",
                Some(json!({
                    "EmailIdentity": identity,
                    "FromAddress": case.from_address,
                    "CertificateArn":
                        "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333",
                })),
            )
            .await,
        ),
        (
            "DisassociateEmailIdentityCertificate",
            ses.call(
                "POST",
                "/v2/email/identity/certificates/delete",
                Some(json!({
                    "EmailIdentity": identity,
                    "FromAddress": case.from_address,
                })),
            )
            .await,
        ),
    ]
}

/// Every implemented operation that resolves an email identity and names the name it did
/// not find unquoted, each answered once for `case`.
async fn unquoting_refusals(ses: &Ses, case: &Case) -> Vec<(&'static str, Reply)> {
    let identity = case.identity;
    vec![
        ("DeleteEmailIdentity", ses.delete_identity(identity).await),
        (
            "PutEmailIdentityDkimAttributes",
            ses.call(
                "PUT",
                &format!("/v2/email/identities/{identity}/dkim"),
                Some(json!({"SigningEnabled": true})),
            )
            .await,
        ),
        (
            "PutEmailIdentityFeedbackAttributes",
            ses.call(
                "PUT",
                &format!("/v2/email/identities/{identity}/feedback"),
                Some(json!({"EmailForwardingEnabled": true})),
            )
            .await,
        ),
        (
            "PutEmailIdentityMailFromAttributes",
            ses.call(
                "PUT",
                &format!("/v2/email/identities/{identity}/mail-from"),
                Some(json!({"MailFromDomain": "mail.example.com"})),
            )
            .await,
        ),
        (
            "PutEmailIdentityConfigurationSetAttributes",
            ses.call(
                "PUT",
                &format!("/v2/email/identities/{identity}/configuration-set"),
                Some(json!({})),
            )
            .await,
        ),
        (
            "CreateEmailIdentityPolicy",
            ses.call(
                "POST",
                &format!("/v2/email/identities/{identity}/policies/Policy1"),
                Some(json!({"Policy": policy_document()})),
            )
            .await,
        ),
    ]
}

#[tokio::test]
async fn every_operation_refuses_a_missing_identity_with_its_own_sentence() {
    let ses = Ses::new();
    // One identity exists, so each refusal is about the name asked for and not about an
    // empty account.
    assert_eq!(
        ses.create_identity("present.example.com").await.status,
        200,
        "creating the identity that does exist must answer 200"
    );

    for case in CASES {
        let identity = case.identity;
        for (operation, reply) in quoting_refusals(&ses, case).await {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (404, "NotFoundException"),
                "case ({operation}, {identity}): expected 404 NotFoundException, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.message(),
                quoted(identity),
                "case ({operation}, {identity}): expected {:?}, got {:?}",
                quoted(identity),
                reply.message()
            );
        }
        for (operation, reply) in unquoting_refusals(&ses, case).await {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (404, "NotFoundException"),
                "case ({operation}, {identity}): expected 404 NotFoundException, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.message(),
                unquoted(identity),
                "case ({operation}, {identity}): expected {:?}, got {:?}",
                unquoted(identity),
                reply.message()
            );
            assert!(
                !reply.message().contains('<') && !reply.message().contains('>'),
                "case ({operation}, {identity}): the sentence must not quote the identity, \
                 got {:?}",
                reply.message()
            );
        }
    }

    // Nothing any refused call did removed the identity that exists.
    let read = ses.get_identity("present.example.com").await;
    assert_eq!(
        read.status, 200,
        "the identity that exists must still be readable, got {} {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn the_delete_and_the_read_of_one_dead_identity_differ_only_in_the_brackets() {
    for case in CASES {
        let ses = Ses::new();
        let identity = case.identity;
        assert_eq!(
            ses.create_identity(identity).await.status,
            200,
            "case {identity}: creating the identity must answer 200"
        );
        let deleted = ses.delete_identity(identity).await;
        assert_eq!(
            deleted.status, 200,
            "case {identity}: deleting the identity it holds must answer 200, got {} {:?}",
            deleted.status, deleted.body
        );

        // The same identity, now dead: the delete names it unquoted, the read quotes it,
        // and both are the same 404 `NotFoundException`.
        let again = ses.delete_identity(identity).await;
        let read = ses.get_identity(identity).await;
        for (operation, reply) in [("DeleteEmailIdentity", &again), ("GetEmailIdentity", &read)] {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (404, "NotFoundException"),
                "case ({operation}, {identity}): expected 404 NotFoundException, got {} {:?}",
                reply.status,
                reply.body
            );
        }
        assert_eq!(
            again.message(),
            unquoted(identity),
            "case {identity}: expected {:?} from the delete, got {:?}",
            unquoted(identity),
            again.message()
        );
        assert_eq!(
            read.message(),
            quoted(identity),
            "case {identity}: expected {:?} from the read, got {:?}",
            quoted(identity),
            read.message()
        );
        assert_eq!(
            read.message(),
            again.message().replacen(
                identity,
                &format!("<{identity}>"),
                1
            ),
            "case {identity}: the two sentences must differ in the brackets alone, got {:?} \
             and {:?}",
            read.message(),
            again.message()
        );
    }
}
