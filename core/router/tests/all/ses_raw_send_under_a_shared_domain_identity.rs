// SPDX-License-Identifier: CC0-1.0

//! Who may send a raw message: the account's identity set, not the API that recorded
//! it.
//!
//! The rule under test: Amazon SES authorises a send when the sender address is itself
//! a verified identity of the account OR sits in a domain the account holds a verified
//! DOMAIN identity for — a verified domain covers every mailbox in it and in its
//! subdomains, so the mailbox never has to be verified on its own — whichever API over
//! the account's one identity set recorded that identity; otherwise the send is refused
//! `MessageRejected` (HTTP 400) naming the Region the request was signed for and the
//! addr-spec that failed the check. A domain is a DNS name, so the comparison that
//! decides which identity covers a sender ignores the case of the domain part.
//!
//! The rows differ along what the rule generalises over:
//!
//! * which API recorded the identity — the v2 REST `CreateEmailIdentity`, the classic
//!   `awsQuery` `VerifyDomainIdentity`, and the Amazon Pinpoint Email API, three
//!   surfaces over ONE account identity set;
//! * how far the sender sits below the identity — the identity's own apex, one
//!   subdomain label below it, two labels below it, and the mailbox itself recorded as
//!   an address identity;
//! * how the request spells the sender — bare, in display-name form, and with the
//!   domain cased differently from the way the identity was recorded;
//! * and the two ways a raw send names its sender — the `Source` parameter, and the
//!   raw message's own `From` line when the request names no `Source`.
//!
//! Two rows carry no covering identity (a sender in an unrelated domain, and an account
//! that recorded nothing at all) and pin the refusal's wording.
//!
//! This suite needs the services over ONE engine, with the other members of the `ses`
//! signing-name family mounted beside them — the same assembly `core/router/src/lib.rs`
//! performs — because the hand-off being asserted exists only when one engine is
//! shared, and the claim that resolves each request to one service exists only when the
//! peers are there to compete for it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// The Region every row signs for, which is the Region a refusal names.
const REGION: &str = "us-east-1";

/// SES v1's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-12-01).
const SES_V1_API_VERSION: &str = "2010-12-01";

/// The two routing refusals the router answers itself, which no row here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none
/// did). Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// How long a row waits for the account's verification of a freshly recorded domain
/// identity to come through, and how often it looks.
///
/// Nothing but the clock moves that record, so a row polls the identity the way a client
/// does rather than reaching into the store; the ceiling only bounds a hung test.
const VERIFICATION_WAIT: Duration = Duration::from_secs(30);

/// The gap between two polls of [`VERIFICATION_WAIT`].
const VERIFICATION_POLL: Duration = Duration::from_millis(50);

/// The MIME boundary the raw messages below delimit their parts with.
const BOUNDARY: &str = "----sim-raw-send-boundary";

/// Which API recorded an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The classic `awsQuery` API (`VerifyDomainIdentity`/`VerifyEmailIdentity`).
    V1,
    /// The v2 REST API (`CreateEmailIdentity`).
    V2,
    /// The Amazon Pinpoint Email API, the third over the same identity set.
    PinpointEmail,
}

/// One row: the identity the account records, the sender a raw send then names, and
/// whether the gate authorises it.
struct Row {
    /// What makes this row different from the others.
    case: &'static str,
    /// The identity the account records, and the API that records it; `None` for a row
    /// whose account records nothing at all.
    recorded: Option<(Surface, &'static str)>,
    /// The `Source` (or `From` line) the send names.
    sender: &'static str,
    /// The addr-spec the gate checks, which is what a refusal names.
    checked: &'static str,
    /// Whether the send is authorised.
    accepted: bool,
}

/// The rows: the classes of covering identity, the spellings of a sender under one, and
/// the two senders nothing covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a mailbox in a domain created with the v2 API",
            recorded: Some((Surface::V2, "raw-apex.example.net")),
            sender: "sender@raw-apex.example.net",
            checked: "sender@raw-apex.example.net",
            accepted: true,
        },
        Row {
            case: "a mailbox one label below a domain created with the v2 API",
            recorded: Some((Surface::V2, "raw-sub.example.net")),
            sender: "sender@mail.raw-sub.example.net",
            checked: "sender@mail.raw-sub.example.net",
            accepted: true,
        },
        Row {
            case: "a mailbox two labels below a domain created with the v2 API",
            recorded: Some((Surface::V2, "raw-deep.example.net")),
            sender: "sender@out.mail.raw-deep.example.net",
            checked: "sender@out.mail.raw-deep.example.net",
            accepted: true,
        },
        Row {
            case: "the mailbox itself created as an address identity with the v2 API",
            recorded: Some((Surface::V2, "solo@raw-address.example.net")),
            sender: "solo@raw-address.example.net",
            checked: "solo@raw-address.example.net",
            accepted: true,
        },
        Row {
            case: "a sender in display-name form under a domain created with the v2 API",
            recorded: Some((Surface::V2, "raw-named.example.net")),
            sender: "\"Raw Sender\" <writer@raw-named.example.net>",
            checked: "writer@raw-named.example.net",
            accepted: true,
        },
        Row {
            case: "a sender whose domain is cased unlike the v2-created identity",
            recorded: Some((Surface::V2, "Raw-MiXeD.Example.NET")),
            sender: "sender@raw-mixed.example.net",
            checked: "sender@raw-mixed.example.net",
            accepted: true,
        },
        Row {
            case: "a mailbox in a domain verified with the classic API",
            recorded: Some((Surface::V1, "raw-classic.example.net")),
            sender: "sender@raw-classic.example.net",
            checked: "sender@raw-classic.example.net",
            accepted: true,
        },
        Row {
            case: "a mailbox in a domain created with the Pinpoint Email API",
            recorded: Some((Surface::PinpointEmail, "raw-pinpoint.example.net")),
            sender: "sender@raw-pinpoint.example.net",
            checked: "sender@raw-pinpoint.example.net",
            accepted: true,
        },
        Row {
            case: "a sender in a domain the account holds no identity for",
            recorded: Some((Surface::V2, "raw-other.example.net")),
            sender: "sender@raw-stranger.example.net",
            checked: "sender@raw-stranger.example.net",
            accepted: false,
        },
        Row {
            case: "a sender under an account that recorded no identity at all",
            recorded: None,
            sender: "sender@raw-empty.example.net",
            checked: "sender@raw-empty.example.net",
            accepted: false,
        },
    ]
}

/// The four services sharing the SigV4 signing name `ses`, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These rows exercise routing and handler behaviour, not SigV4.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_pinpointemail_amazonpinpointemailservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and
/// [`REGION`] — the signals the router resolves a routing key from and the handler reads
/// the Region out of. The signature itself is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answered request.
struct Answer {
    status: u16,
    /// The body as text.
    text: String,
    /// The body parsed as JSON, or `Value::Null` for the XML legs.
    json: Value,
}

/// Send one request, refusing at the door the two answers the ROUTER itself produces: a
/// request that reached nobody, or one several services claimed.
async fn send(
    router: &MultiServiceRouter,
    case: &str,
    leg: &str,
    request: http::Request<Bytes>,
) -> Answer {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(response.body()).to_string();
    for refusal in ROUTER_REFUSALS {
        assert!(
            !text.contains(refusal) && !error_type.contains(refusal),
            "case {case:?}: {leg} must be served by exactly one service, got HTTP {status}: \
             {text}"
        );
    }
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, json }
}

/// One classic-API call: `awsQuery` under the `ses` credential scope, `POST /` with the
/// operation in the `Action` form field.
async fn v1(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    fields: &[(&str, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version={SES_V1_API_VERSION}");
    for (name, value) in fields {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://email.{REGION}.amazonaws.com/"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization("ses"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    send(router, case, &format!("SES v1 {action}"), request).await
}

/// [`v1`] where the call must succeed.
async fn v1_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    fields: &[(&str, String)],
) -> String {
    let answered = v1(router, case, action, fields).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SES v1 {action} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.text
}

/// One REST call in a `restJson1` surface's own path space, under the `ses` credential
/// scope.
async fn rest(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization("ses"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    send(router, case, &format!("{method} {path}"), request).await
}

/// [`rest`] where the call must succeed.
async fn rest_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let answered = rest(router, case, method, path, body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: {method} {path} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.json
}

/// A JSON object's string member, or the empty string.
fn text_of(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Record `identity` through `surface`, and wait until the ACCOUNT reports the
/// verification as having come through.
///
/// Recording an identity only STARTS its verification, so the row waits for the move the
/// clock owes the record the way a client does — polling the v2 read — instead of
/// reaching into the store. The v2 read is used for every surface because it reports both
/// halves of the state a send is gated on: whether the identity is verified for sending,
/// and where the search for its Easy DKIM records stands.
async fn record_and_await_verification(
    router: &MultiServiceRouter,
    case: &str,
    surface: Surface,
    identity: &str,
) {
    match surface {
        Surface::V2 => {
            rest_ok(
                router,
                case,
                "POST",
                "/v2/email/identities",
                Some(json!({"EmailIdentity": identity})),
            )
            .await;
        }
        Surface::PinpointEmail => {
            rest_ok(
                router,
                case,
                "POST",
                "/v1/email/identities",
                Some(json!({"EmailIdentity": identity})),
            )
            .await;
        }
        Surface::V1 => {
            let action = if identity.contains('@') {
                ("VerifyEmailIdentity", "EmailAddress")
            } else {
                ("VerifyDomainIdentity", "Domain")
            };
            v1_ok(
                router,
                case,
                action.0,
                &[(action.1, identity.to_string())],
            )
            .await;
        }
    }

    let deadline = std::time::Instant::now() + VERIFICATION_WAIT;
    let mut last = Value::Null;
    while std::time::Instant::now() < deadline {
        last = rest_ok(
            router,
            case,
            "GET",
            &format!("/v2/email/identities/{identity}"),
            None,
        )
        .await;
        let verified = last
            .get("VerifiedForSendingStatus")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if verified {
            // A domain identity the recording API set Easy DKIM up for is verified
            // by those records, so the account reports that search as having
            // succeeded too — the state the recorded scenario waits for before it
            // sends. The classic API sets Easy DKIM up in a call of its own
            // (`VerifyDomainDkim`), so a domain it merely verified holds no setup to
            // report.
            let sets_up_easy_dkim =
                !identity.contains('@') && matches!(surface, Surface::V2 | Surface::PinpointEmail);
            if !sets_up_easy_dkim {
                return;
            }
            let dkim = last
                .get("DkimAttributes")
                .map(|attributes| text_of(attributes, "Status"))
                .unwrap_or_default();
            assert_eq!(
                dkim, "SUCCESS",
                "case {case:?}: expected the verified domain's DKIM search to have succeeded, \
                 got {last}"
            );
            return;
        }
        tokio::time::sleep(VERIFICATION_POLL).await;
    }
    panic!(
        "case {case:?}: {identity} was not verified for sending within {VERIFICATION_WAIT:?}, \
         last read {last}"
    );
}

/// A multipart/mixed raw message from `sender`, carrying a text part and a binary
/// attachment — the shape of message `SendRawEmail` exists for.
fn raw_message(sender: &str, recipient: &str, subject: &str) -> Vec<u8> {
    let attachment: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
    let encoded = ::aws_smithy_types::base64::encode(&attachment);
    format!(
        "From: {sender}\r\n\
         To: {recipient}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"{BOUNDARY}\"\r\n\
         \r\n\
         --{BOUNDARY}\r\n\
         Content-Type: text/plain; charset=us-ascii\r\n\
         \r\n\
         The attachment is the payload.\r\n\
         --{BOUNDARY}\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Transfer-Encoding: base64\r\n\
         Content-Disposition: attachment; filename=\"probe.bin\"\r\n\
         \r\n\
         {encoded}\r\n\
         --{BOUNDARY}--\r\n"
    )
    .into_bytes()
}

/// The two ways one raw send names its sender: the `Source` parameter, and the raw
/// message's own `From` line when the request names none — "If you do not provide a value
/// for this parameter, you must specify a 'From' address in the raw text of the message"
/// (`SendRawEmailRequest$Source`).
fn raw_sends(sender: &str, subject: &str) -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    const RECIPIENT: &str = "recipient@example.org";
    let data = ::aws_smithy_types::base64::encode(raw_message(sender, RECIPIENT, subject));
    vec![
        (
            "SendRawEmail naming a Source",
            vec![
                ("RawMessage.Data", data.clone()),
                ("Source", sender.to_string()),
                ("Destinations.member.1", RECIPIENT.to_string()),
            ],
        ),
        (
            "SendRawEmail reading the message's own From line",
            vec![("RawMessage.Data", data)],
        ),
    ]
}

/// The message a send from an identity the account does not hold verified is refused
/// with: fixed text, the Region the request was signed for upper-cased, and the
/// identities from the request.
fn unverified_message(identity: &str) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {identity}",
        REGION.to_uppercase()
    )
}

/// `GetSendQuota`'s `SentLast24Hours`, which is DERIVED from the account's send log: it
/// is how a row reads back whether an accepted send reached it.
async fn sent_last_24_hours(router: &MultiServiceRouter, case: &str) -> f64 {
    let body = v1_ok(router, case, "GetSendQuota", &[]).await;
    let text = opt_element(&body, "SentLast24Hours")
        .unwrap_or_else(|| panic!("case {case:?}: expected SentLast24Hours, got {body}"));
    text.parse::<f64>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a number, got {text:?}"))
}

#[tokio::test]
async fn a_raw_send_is_authorised_by_the_account_s_identity_whichever_api_recorded_it() {
    for row in rows() {
        let case = row.case;
        let router = router();
        if let Some((surface, identity)) = row.recorded {
            record_and_await_verification(&router, case, surface, identity).await;
        }

        let mut accepted = 0usize;
        for (leg, fields) in raw_sends(row.sender, "A raw multipart probe") {
            let answered = v1(&router, case, "SendRawEmail", &fields).await;
            if row.accepted {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?} through {leg}: expected HTTP 200, got {} with {}",
                    answered.status, answered.text
                );
                let message_id = opt_element(&answered.text, "MessageId").unwrap_or_default();
                assert!(
                    !message_id.is_empty(),
                    "case {case:?} through {leg}: an accepted send reports the id it stored, \
                     got {}",
                    answered.text
                );
                accepted += 1;
                continue;
            }
            assert_eq!(
                answered.status, 400,
                "case {case:?} through {leg}: expected HTTP 400, got {} with {}",
                answered.status, answered.text
            );
            let code = opt_element(&answered.text, "Code").unwrap_or_default();
            assert_eq!(
                code, "MessageRejected",
                "case {case:?} through {leg}: expected the modelled MessageRejected, got \
                 {code:?} in {}",
                answered.text
            );
            // Read through the shared repeated-element reader, which undoes the
            // serializer's escaping, so the message compares as the service worded it.
            let stated = elements(&answered.text, "Message")
                .into_iter()
                .next()
                .unwrap_or_default();
            let expected = unverified_message(row.checked);
            assert_eq!(
                stated, expected,
                "case {case:?} through {leg}: expected {expected:?}, got {stated:?}"
            );
        }

        // What the gate let through is what the account's send log holds: one counted
        // recipient per accepted send, and nothing at all for a refused one.
        let sent = sent_last_24_hours(&router, case).await;
        assert_eq!(
            sent, accepted as f64,
            "case {case:?}: expected {accepted} counted recipients in the send log, got {sent}"
        );
    }
}
