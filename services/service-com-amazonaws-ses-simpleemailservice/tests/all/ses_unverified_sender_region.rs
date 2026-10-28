// SPDX-License-Identifier: CC0-1.0

//! The `MessageRejected` a send is refused with when the account has not verified the
//! identity it is sending from, driven end to end over the awsQuery wire.
//!
//! One rule, asserted over a SET of inputs rather than the one a report cited: SES
//! identities are per-REGION — an identity verified in one Region is not verified in
//! another — so the refusal NAMES the Region the verification check ran in, upper cased,
//! before the identities that failed it:
//!
//! ```text
//! Email address is not verified. The following identities failed the check in region \
//! <REGION>: <identity>
//! ```
//!
//! Everything but the Region token and the identity list is fixed text; the Region comes
//! from the request's SigV4 credential scope and the identity from the request. The rows
//! differ along what the rule generalises over: which Region the request is scoped to
//! (several, including one whose label the emulator falls back to when the request is
//! scoped to none), the case the Region label arrives in, and the form of the sending
//! identity (a mailbox, a mailbox of a domain this account verified in ANOTHER Region, a
//! sub-domain mailbox).
//!
//! The same rule's other half is asserted too: an identity the account HAS verified in
//! the Region the request is scoped to is not refused at all, so the message is about the
//! gate and not about every send.

use service_com_amazonaws_ses_simpleemailservice::state::DEFAULT_REGION;
use service_com_amazonaws_ses_simpleemailservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use crate::harness::{element, field, form_encode, settle_verifications, xml_text, Field, VERSION};

/// The message a send from an unverified identity is refused with, as this service words
/// it for `region` and `identities`.
fn expected_message(region: &str, identities: &[&str]) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {}",
        region.to_uppercase(),
        identities.join(", ")
    )
}

/// A router holding only this service, over a fresh engine, and that engine.
fn emulator_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A router holding only this service, over a fresh engine.
fn emulator() -> MultiServiceRouter {
    emulator_and_engine().0
}

/// One awsQuery call whose SigV4 credential scope names `region`, or that carries no
/// `Authorization` header at all when `region` is `None` — which is how a request scoped
/// to no Region reaches the service.
///
/// The harness's own `call` pins one Region in its header, and the Region is exactly what
/// these rows vary, so the request is built here instead.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: Option<&str>,
    action: &str,
    params: &[Field],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = match region {
        Some(region) if region.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') => {
            format!("email.{region}.amazonaws.com")
        }
        // A scope naming no Region label, and a request carrying no scope at all, still
        // arrive at this service's own endpoint — which is what claims them.
        _ => "email.us-east-1.amazonaws.com".to_string(),
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(region) = region {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ses/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body))
                .expect("the request builds"),
        )
        .await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// A `SendEmail` request from `source` to one recipient.
fn send_from(source: &str) -> Vec<Field> {
    vec![
        field("Source", source),
        field("Destination.ToAddresses.member.1", "recipient@example.net"),
        field("Message.Subject.Data", "Quarterly report"),
        field("Message.Body.Text.Data", "See attached."),
    ]
}

/// One row: the Region the request is scoped to, the sending identity, and the Region
/// token the refusal must name.
struct Row {
    case: &'static str,
    /// The Region of the request's credential scope, or `None` for a request carrying no
    /// scope at all.
    scoped_to: Option<&'static str>,
    /// The Region label the message must name, before upper casing.
    reported: &'static str,
    source: &'static str,
}

#[tokio::test]
async fn an_unverified_sender_is_refused_with_the_region_the_check_ran_in() {
    let rows = vec![
        Row {
            case: "a request scoped to us-west-2",
            scoped_to: Some("us-west-2"),
            reported: "us-west-2",
            source: "allowed@example.com",
        },
        Row {
            case: "the same send scoped to another Region",
            scoped_to: Some("eu-central-1"),
            reported: "eu-central-1",
            source: "allowed@example.com",
        },
        Row {
            case: "a Region label with no digit in it",
            scoped_to: Some("sa-east-1"),
            reported: "sa-east-1",
            source: "allowed@example.com",
        },
        Row {
            case: "a request carrying no credential scope, which the endpoint resolves to \
                   the default Region",
            scoped_to: None,
            reported: DEFAULT_REGION,
            source: "allowed@example.com",
        },
        Row {
            case: "a scope whose Region field is no Region label, which the endpoint \
                   resolves the same way",
            scoped_to: Some("NOT A REGION"),
            reported: DEFAULT_REGION,
            source: "allowed@example.com",
        },
        Row {
            case: "a sub-domain mailbox as the sending identity",
            scoped_to: Some("ap-southeast-2"),
            reported: "ap-southeast-2",
            source: "noreply@mail.example.com",
        },
        Row {
            case: "a mailbox whose local part carries the characters an address may hold",
            scoped_to: Some("us-east-2"),
            reported: "us-east-2",
            source: "first.last+tag@example.org",
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();
        let (status, body) =
            call_in_region(&router, row.scoped_to, "SendEmail", &send_from(row.source)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = element(&body, "Code").unwrap_or_default();
        assert_eq!(
            code, "MessageRejected",
            "case {case:?}: expected the modelled MessageRejected, got {code:?} in {body}"
        );
        let stated = element(&body, "Message")
            .map(|raw| xml_text(&raw))
            .unwrap_or_default();
        let expected = expected_message(row.reported, &[row.source]);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_identity_verified_in_the_requests_region_is_not_refused() {
    // The other half of the same rule: the gate is what the message reports, so an
    // identity this account verified is accepted whatever Region the send is scoped to —
    // the emulator serves one Region's identity list, the one every call reads.
    for region in [Some("us-west-2"), Some("eu-central-1"), None] {
        let (router, engine) = emulator_and_engine();
        let (status, body) = call_in_region(
            &router,
            region,
            "VerifyEmailIdentity",
            &[field("EmailAddress", "allowed@example.com")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: VerifyEmailIdentity expected 200, got {status} with {body}"
        );
        // The request only ASKS for the verification; what these rows are about is
        // which Region the refusal names, so the store is settled past the span the
        // clock owes the record rather than slept through.
        settle_verifications(&engine);

        let (status, body) = call_in_region(
            &router,
            region,
            "SendEmail",
            &send_from("allowed@example.com"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: a verified sender expected 200, got {status} with {body}"
        );
        assert!(
            element(&body, "MessageId").is_some(),
            "case {region:?}: an accepted send reports the id it stored, got {body}"
        );
    }
}
