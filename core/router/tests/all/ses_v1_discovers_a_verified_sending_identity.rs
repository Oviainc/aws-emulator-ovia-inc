// SPDX-License-Identifier: CC0-1.0

//! Finding the account's verified SENDER through the classic SES identity plane.
//!
//! The rule under test: **an email identity belongs to the ACCOUNT, so whichever of the
//! account's email APIs recorded it, once the verification the clock owes it is due the
//! classic `awsQuery` plane enumerates it with `ListIdentities` under its own
//! `IdentityType` and reports it with `VerificationStatus` `Success` — carrying the
//! stored TXT `VerificationToken` for a domain identity and none for an address
//! identity — in agreement with what the v2 plane reports for that same identity
//! (`VerificationStatus` `SUCCESS`, `VerifiedForSendingStatus` true); and a
//! `FromEmailAddress` that identity covers is then admitted by
//! `CreateCustomVerificationEmailTemplate`, which echoes it back on
//! `GetCustomVerificationEmailTemplate`.**
//!
//! That is the pre-flight every SES client performs before it sends or before it creates
//! anything gated on sender verification: page the identity listing per class, read the
//! verification of what was listed, and keep the identities that came back verified.
//!
//! Two crates have to agree for the rule to hold — the identity is recorded on one SES
//! surface and discovered on the other — so the suite lives here: a crate-local test only
//! ever sees one side of it.
//!
//! The rows differ along what the rule generalises over: which surface RECORDED the
//! identity, the class of identity (an apex domain, a subdomain, a mailbox), the form of
//! the name (mixed case, a plus-tagged local part) and the sender form the identity covers
//! (the mailbox itself, a mailbox under the domain). The identity an issue happens to
//! mention is one row's value.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, entries_of, form_encode, members_of, opt_element};

/// The Region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// SES v1's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-12-01).
const SES_V1_API_VERSION: &str = "2010-12-01";

/// The state the classic plane reports for an identity the account accepts as a sender
/// (`IdentityVerificationAttributes$VerificationStatus`, whose value set is
/// `Pending, Success, Failed, TemporaryFailure, NotStarted`).
const SUCCESS: &str = "Success";

/// The v2 plane's spelling of that same state (`VerificationStatus$SUCCESS`).
const SUCCESS_V2: &str = "SUCCESS";

/// The decoded length of a domain identity's TXT verification token, as the account's
/// shared identity registry mints it (`service_common::ses_identities`).
const VERIFICATION_TOKEN_BYTES: usize = 32;

/// The page size the discovery walk asks `ListIdentities` for, small enough that the
/// account's identities of one class do not fit on one page: the walk follows `NextToken`
/// to the end, which is what a client paging the listing does.
const PAGE_SIZE: usize = 2;

/// The identities one `GetIdentityVerificationAttributes` call may name, from that
/// operation's own documentation: it "can only get verification attributes for up to 100
/// identities at a time".
const IDENTITIES_PER_READ: usize = 100;

/// How long the verification poll waits, and how often it looks.
///
/// The identity's pending -> verified move is owed on the emulator's own accelerated
/// schedule, so what is waited for takes a few seconds; the deadline is only here so a
/// failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);

/// The two routing refusals the router answers itself, which no row here may provoke.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// Which of the account's email APIs recorded an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The classic `awsQuery` API.
    V1,
    /// The v2 REST API.
    V2,
}

/// One row: an identity, the API that records it, and a sender it covers.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The identity as the recording request spells it.
    identity: &'static str,
    /// The API that records it.
    recorded_on: Surface,
    /// The classic plane's `IdentityType` for it.
    identity_type: &'static str,
    /// A `FromEmailAddress` that identity covers.
    sender: &'static str,
}

/// The rows, spanning both recording surfaces and the identity and sender forms.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an apex domain created with the v2 API",
            identity: "v2-apex.example.com",
            recorded_on: Surface::V2,
            identity_type: "Domain",
            sender: "noreply@v2-apex.example.com",
        },
        Row {
            case: "a subdomain created with the v2 API",
            identity: "mail.v2-sub.example.org",
            recorded_on: Surface::V2,
            identity_type: "Domain",
            sender: "bounce@mail.v2-sub.example.org",
        },
        Row {
            case: "a mailbox created with the v2 API",
            identity: "sender@v2-address.example.net",
            recorded_on: Surface::V2,
            identity_type: "EmailAddress",
            sender: "sender@v2-address.example.net",
        },
        Row {
            case: "a mixed-case domain created with the v2 API",
            identity: "V2-MiXeD.Example.COM",
            recorded_on: Surface::V2,
            identity_type: "Domain",
            sender: "noreply@V2-MiXeD.Example.COM",
        },
        Row {
            case: "a domain verified with the classic API",
            identity: "v1-apex.example.com",
            recorded_on: Surface::V1,
            identity_type: "Domain",
            sender: "noreply@v1-apex.example.com",
        },
        Row {
            case: "a plus-tagged mailbox verified with the classic API",
            identity: "ops+ses@v1-address.example.net",
            recorded_on: Surface::V1,
            identity_type: "EmailAddress",
            sender: "ops+ses@v1-address.example.net",
        },
    ]
}

/// The members of the `ses` signing-name family, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the hand-off being asserted exists only when the
/// engine is shared, and the claim that resolves each request to one service exists only
/// when the peers are there to compete for it.
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
    router
        .register_service(service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService::new(
            engine,
        ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names the `ses` signing name —
/// the signal the router resolves a routing key from. The signature itself is never
/// checked.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ses/aws4_request, \
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
        .header("authorization", authorization())
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

/// One v2-API call: `restJson1` under the `ses` credential scope, in the `/v2/email/…`
/// path space. The call must succeed.
async fn v2_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let answered = send(router, case, &format!("SESv2 {method} {path}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SESv2 {method} {path} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.json
}

/// What the classic plane reports about one identity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    /// The identity, in the spelling the account holds it under.
    identity: String,
    /// The `IdentityType` the listing filed it under.
    identity_type: String,
    /// Its `VerificationStatus`.
    status: String,
    /// Its TXT `VerificationToken`, which only a domain identity carries.
    token: Option<String>,
}

/// Every identity the classic plane lists for the class `identity_type`, following its
/// cursor to the end.
async fn listed(router: &MultiServiceRouter, case: &str, identity_type: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut fields: Vec<(&str, String)> = vec![
            ("IdentityType", identity_type.to_string()),
            ("MaxItems", PAGE_SIZE.to_string()),
        ];
        if let Some(token) = &token {
            fields.push(("NextToken", token.clone()));
        }
        let body = v1_ok(router, case, "ListIdentities", &fields).await;
        let page = opt_element(&body, "Identities").unwrap_or_default();
        let members = members_of(&page);
        assert!(
            members.len() <= PAGE_SIZE,
            "case {case:?}: a page of {PAGE_SIZE} may carry at most that many identities, got \
             {page}"
        );
        seen.extend(members.into_iter().map(str::to_string));
        pages += 1;
        token = opt_element(&body, "NextToken");
        if token.is_none() {
            break;
        }
        assert!(
            pages <= IDENTITIES_PER_READ,
            "case {case:?}: ListIdentities(IdentityType={identity_type:?}) did not terminate, \
             saw {seen:?}"
        );
    }
    seen
}

/// The pre-flight an SES client performs to find a sender, served entirely by the classic
/// plane: page the listing for each class the model declares, then read the verification of
/// everything it listed, in batches no larger than that read's documented bound.
async fn discover(router: &MultiServiceRouter, case: &str) -> Vec<Reported> {
    let mut reported: Vec<Reported> = Vec::new();
    // The classes are the model's own `IdentityType` value set, not a list written here:
    // `IDENTITY_TYPE_VALUES` is generated from the model.
    for identity_type in service_com_amazonaws_ses_simpleemailservice::ops::IDENTITY_TYPE_VALUES {
        let identities = listed(router, case, identity_type).await;
        for batch in identities.chunks(IDENTITIES_PER_READ) {
            // One form key per element of the `Identities` list, which an awsQuery body
            // spells `Identities.member.<n>`.
            let keys: Vec<String> = (1..=batch.len())
                .map(|index| format!("Identities.member.{index}"))
                .collect();
            let mut fields: Vec<(&str, String)> = Vec::new();
            for (key, identity) in keys.iter().zip(batch) {
                fields.push((key.as_str(), identity.clone()));
            }
            let body =
                v1_ok(router, case, "GetIdentityVerificationAttributes", &fields).await;
            let attributes = block_of(&body, "VerificationAttributes").unwrap_or_default();
            for (identity, value) in entries_of(attributes) {
                reported.push(Reported {
                    identity,
                    identity_type: (*identity_type).to_string(),
                    status: opt_element(value, "VerificationStatus").unwrap_or_default(),
                    token: opt_element(value, "VerificationToken"),
                });
            }
        }
    }
    reported
}

/// The entry the walk reported for `identity`, by the spelling it was recorded with.
fn entry_for<'a>(reported: &'a [Reported], identity: &str) -> Option<&'a Reported> {
    reported.iter().find(|entry| entry.identity == identity)
}

/// The six members `CreateCustomVerificationEmailTemplate` carries, in the order the model
/// lists them.
fn create_fields(name: &str, sender: &str) -> Vec<(&'static str, String)> {
    vec![
        ("TemplateName", name.to_string()),
        ("FromEmailAddress", sender.to_string()),
        ("TemplateSubject", "Please verify your address".to_string()),
        (
            "TemplateContent",
            "<html><body>Please verify your address.</body></html>".to_string(),
        ),
        (
            "SuccessRedirectionURL",
            "https://example.com/verified".to_string(),
        ),
        (
            "FailureRedirectionURL",
            "https://example.com/failed".to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_identity_the_account_verified_is_discovered_as_a_sender_through_the_classic_plane() {
    let router = router();

    // Record every row, each through its own surface.
    for row in rows() {
        let case = row.case;
        match (row.recorded_on, row.identity_type) {
            (Surface::V2, _) => {
                v2_ok(
                    &router,
                    case,
                    "POST",
                    "/v2/email/identities",
                    Some(json!({"EmailIdentity": row.identity})),
                )
                .await;
            }
            (Surface::V1, "Domain") => {
                v1_ok(
                    &router,
                    case,
                    "VerifyDomainIdentity",
                    &[("Domain", row.identity.to_string())],
                )
                .await;
            }
            (Surface::V1, _) => {
                v1_ok(
                    &router,
                    case,
                    "VerifyEmailIdentity",
                    &[("EmailAddress", row.identity.to_string())],
                )
                .await;
            }
        }
    }

    // Recording an identity only STARTS its verification, so the walk is repeated until
    // the move the clock owes every one of them has fallen due. The deadline is what
    // turns "no identity ever reads verified" into a failure rather than a hang.
    let deadline = Instant::now() + DEADLINE;
    let discovered = loop {
        let discovered = discover(&router, "the whole account").await;
        let verified = discovered
            .iter()
            .filter(|entry| entry.status == SUCCESS)
            .count();
        if verified == rows().len() {
            break discovered;
        }
        assert!(
            Instant::now() < deadline,
            "case the whole account: every identity the account holds must reach {SUCCESS}, last \
             walk {discovered:?}"
        );
        tokio::time::sleep(POLL).await;
    };

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let Some(entry) = entry_for(&discovered, row.identity) else {
            panic!(
                "case {case:?}: expected the classic plane to report {:?}, got {discovered:?}",
                row.identity
            );
        };
        assert_eq!(
            (entry.identity_type.as_str(), entry.status.as_str()),
            (row.identity_type, SUCCESS),
            "case {case:?}: expected {:?} under IdentityType {:?} reading {SUCCESS}, got {entry:?}",
            row.identity,
            row.identity_type
        );
        // "The verification token for a domain identity. Null for email address
        // identities" (`IdentityVerificationAttributes$VerificationToken`).
        match row.identity_type {
            "Domain" => {
                let token = entry.token.clone().unwrap_or_default();
                let decoded = ::aws_smithy_types::base64::decode(&token)
                    .map(|bytes| bytes.len())
                    .unwrap_or_default();
                assert_eq!(
                    decoded, VERIFICATION_TOKEN_BYTES,
                    "case {case:?}: expected a TXT token decoding to {VERIFICATION_TOKEN_BYTES} \
                     bytes, got {token:?}"
                );
            }
            _ => assert!(
                entry.token.is_none(),
                "case {case:?}: an address identity carries no VerificationToken, got {:?}",
                entry.token
            ),
        }

        // The v2 plane answers for the SAME account identity, in its own spelling, and
        // reports it usable as a sender.
        let deadline = Instant::now() + DEADLINE;
        let read = loop {
            let read = v2_ok(
                &router,
                case,
                "GET",
                &format!("/v2/email/identities/{}", row.identity),
                None,
            )
            .await;
            if read["VerifiedForSendingStatus"] == json!(true) {
                break read;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the identity {:?} must reach VerifiedForSendingStatus true, last \
                 read {read}",
                row.identity
            );
            tokio::time::sleep(POLL).await;
        };
        assert_eq!(
            read["VerificationStatus"],
            json!(SUCCESS_V2),
            "case {case:?}: expected both planes to report one verification state, got {SUCCESS} \
             and {read}"
        );

        // And a sender that identity covers is admitted by the operation gated on sender
        // verification, which echoes it back.
        let name = format!("discovered-sender-{index}");
        let created = v1(
            &router,
            case,
            "CreateCustomVerificationEmailTemplate",
            &create_fields(&name, row.sender),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 for the sender {:?} of a verified identity, got {}: {}",
            row.sender, created.status, created.text
        );
        let read = v1_ok(
            &router,
            case,
            "GetCustomVerificationEmailTemplate",
            &[("TemplateName", name.clone())],
        )
        .await;
        assert_eq!(
            opt_element(&read, "FromEmailAddress").as_deref(),
            Some(row.sender),
            "case {case:?}: expected the read of {name:?} to echo the sender {:?}, got {read}",
            row.sender
        );
    }
}
