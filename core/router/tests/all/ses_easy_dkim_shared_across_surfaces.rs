// SPDX-License-Identifier: CC0-1.0

//! ONE Easy DKIM setup behind the account's email APIs.
//!
//! The rule under test: the Easy DKIM setup of an identity — the tokens its owner
//! publishes in DNS, whether the service has found them, and whether messages from the
//! identity are signed — belongs to the ACCOUNT's identity, not to the API that recorded
//! it. So a setup recorded through any one of the account's email APIs is the setup every
//! one of them reports: the SAME token set, the same signing flag, and the same state in
//! each API's own spelling (the classic surface's `Pending` for the v2 surface's
//! `PENDING`). A client that creates a domain identity with the modern API and reads the
//! CNAME records to publish with the legacy one is handed the records it must publish,
//! and asking either API again never hands out a second set.
//!
//! The rows differ along what the rule generalises over: which surface RECORDS the setup
//! (the v2 REST API's `CreateEmailIdentity`, the classic API's `VerifyDomainDkim`, and the
//! Amazon Pinpoint Email API's `CreateEmailIdentity`, a third API over the same
//! identities), and the form of the domain the setup is for (an apex, a subdomain, a
//! hyphenated label, a mixed-case name — a DNS name is compared without regard to case).
//!
//! This suite needs the services over ONE engine, with the other members of the `ses`
//! signing-name family mounted beside them — the same assembly `core/router/src/lib.rs`
//! performs — because the hand-off being asserted exists only when one engine is shared,
//! and the claim that resolves each request to one service exists only when the peers are
//! there to compete for it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, entries_of, form_encode, members_of, opt_element};

/// The Region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// SES v1's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-12-01).
const SES_V1_API_VERSION: &str = "2010-12-01";

/// How many CNAME tokens an Easy DKIM setup carries.
///
/// `DkimAttributes$Tokens` (sesv2) is documented as "a set of unique strings that you use
/// to create a set of CNAME records that you add to the DNS configuration for your
/// domain", and the classic API's `VerifyDomainDkim` hands out the same set; AWS publishes
/// three.
const EASY_DKIM_TOKEN_COUNT: usize = 3;

/// The two routing refusals the router answers itself, which no row here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none
/// did). Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// Which API records the Easy DKIM setup of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The classic `awsQuery` API's `VerifyDomainDkim`.
    V1,
    /// The v2 REST API's `CreateEmailIdentity`, which sets Easy DKIM up for a domain.
    V2,
    /// The Amazon Pinpoint Email API's `CreateEmailIdentity`, which does the same.
    PinpointEmail,
}

/// One row: a domain, and the API that sets Easy DKIM up for it.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The domain as the recording request spells it.
    domain: &'static str,
    /// The API that records the setup.
    recorded_on: Surface,
}

/// The rows, spanning every surface that records a setup and the domain forms.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an apex domain created with the v2 API",
            domain: "v2-apex.example.com",
            recorded_on: Surface::V2,
        },
        Row {
            case: "a subdomain created with the v2 API",
            domain: "mail.v2-sub.example.org",
            recorded_on: Surface::V2,
        },
        Row {
            case: "a mixed-case domain created with the v2 API",
            domain: "V2-MiXeD.Example.COM",
            recorded_on: Surface::V2,
        },
        Row {
            case: "an apex domain set up with the classic API",
            domain: "v1-apex.example.com",
            recorded_on: Surface::V1,
        },
        Row {
            case: "a hyphenated subdomain set up with the classic API",
            domain: "mail-one.v1-sub.example.net",
            recorded_on: Surface::V1,
        },
        Row {
            case: "a domain created with the Pinpoint Email API",
            domain: "pinpoint-apex.example.com",
            recorded_on: Surface::PinpointEmail,
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

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — the
/// signal the router resolves a routing key from. The signature itself is never checked.
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
            "case {case:?}: {leg} must be served by exactly one service, got HTTP {status}: {text}"
        );
    }
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, json }
}

/// One classic-API call: `awsQuery` under the `ses` credential scope, `POST /` with the
/// operation in the `Action` form field. The call must succeed.
async fn v1(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    fields: &[(&str, String)],
) -> String {
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
    let answered = send(router, case, &format!("SES v1 {action}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SES v1 {action} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.text
}

/// One REST call — `restJson1` under the `ses` credential scope, in the path space of the
/// API that serves it — which must succeed.
async fn rest(
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
        .header("authorization", authorization("ses"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let answered = send(router, case, &format!("{method} {path}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: {method} {path} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.json
}

/// What one API reports about an identity's Easy DKIM setup.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    /// Whether messages from the identity are signed (`DkimEnabled` / `SigningEnabled`).
    signing_enabled: bool,
    /// The setup's state, in the API's own spelling.
    status: String,
    /// The tokens the owner publishes, in the order the API reports them.
    tokens: Vec<String>,
}

/// The classic API's `GetIdentityDkimAttributes` report for `identity`, and `None` when
/// the required map carries no entry for it at all.
async fn v1_dkim(router: &MultiServiceRouter, case: &str, identity: &str) -> Option<Reported> {
    let answered = v1(
        router,
        case,
        "GetIdentityDkimAttributes",
        &[("Identities.member.1", identity.to_string())],
    )
    .await;
    let attributes = block_of(&answered, "DkimAttributes")?;
    let (_, entry) = entries_of(attributes)
        .into_iter()
        .find(|(key, _)| key == identity)?;
    Some(Reported {
        signing_enabled: opt_element(entry, "DkimEnabled").as_deref() == Some("true"),
        status: opt_element(entry, "DkimVerificationStatus").unwrap_or_default(),
        tokens: block_of(entry, "DkimTokens")
            .map(|list| members_of(list).into_iter().map(str::to_string).collect())
            .unwrap_or_default(),
    })
}

/// The same report out of one of the REST APIs' `DkimAttributes` object.
fn rest_dkim(reported: &Value) -> Reported {
    let dkim = reported.get("DkimAttributes").cloned().unwrap_or(Value::Null);
    Reported {
        signing_enabled: dkim
            .get("SigningEnabled")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        status: dkim
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tokens: dkim
            .get("Tokens")
            .and_then(Value::as_array)
            .map(|tokens| {
                tokens
                    .iter()
                    .map(|token| token.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Whether the two APIs' spellings name the same state: the classic API spells it in
/// TitleCase and the REST APIs in SCREAMING_CASE, so the two reports agree exactly when
/// one is the other's spelling.
fn states_agree(classic: &str, rest: &str) -> bool {
    let expected = match classic {
        "Pending" => "PENDING",
        "Success" => "SUCCESS",
        "Failed" => "FAILED",
        "TemporaryFailure" => "TEMPORARY_FAILURE",
        "NotStarted" => "NOT_STARTED",
        _ => return false,
    };
    expected == rest
}

/// Whether `token` has the shape of an Easy DKIM token: the published length, and the
/// characters a DNS label may carry, since the caller turns it into a CNAME record's name.
fn is_token_shaped(token: &str) -> bool {
    token.len() == 32
        && token
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

/// Record the Easy DKIM setup of `row`'s domain on the API the row names, and answer the
/// tokens that API handed out.
async fn record_setup(router: &MultiServiceRouter, row: &Row) -> Vec<String> {
    let case = row.case;
    match row.recorded_on {
        Surface::V2 => {
            let created = rest(
                router,
                case,
                "POST",
                "/v2/email/identities",
                Some(json!({"EmailIdentity": row.domain})),
            )
            .await;
            rest_dkim(&created).tokens
        }
        Surface::PinpointEmail => {
            let created = rest(
                router,
                case,
                "POST",
                "/v1/email/identities",
                Some(json!({"EmailIdentity": row.domain})),
            )
            .await;
            rest_dkim(&created).tokens
        }
        Surface::V1 => {
            let answered = v1(
                router,
                case,
                "VerifyDomainDkim",
                &[("Domain", row.domain.to_string())],
            )
            .await;
            block_of(&answered, "DkimTokens")
                .map(|list| members_of(list).into_iter().map(str::to_string).collect())
                .unwrap_or_default()
        }
    }
}

#[tokio::test]
async fn an_easy_dkim_setup_is_one_token_set_on_every_surface() {
    let router = router();
    let mut every_token: Vec<(String, String)> = Vec::new();

    for row in rows() {
        let case = row.case;
        let tokens = record_setup(&router, &row).await;
        assert_eq!(
            tokens.len(),
            EASY_DKIM_TOKEN_COUNT,
            "case {case:?}: expected {EASY_DKIM_TOKEN_COUNT} tokens from the API that recorded \
             the setup, got {tokens:?}"
        );
        for token in &tokens {
            assert!(
                is_token_shaped(token),
                "case {case:?}: expected a 32-character lowercase alphanumeric token, got \
                 {token:?}"
            );
            if let Some((owner, _)) = every_token.iter().find(|(_, held)| held == token) {
                panic!("case {case:?}: token {token:?} was already handed to {owner:?}");
            }
            every_token.push((row.domain.to_string(), token.clone()));
        }

        // The classic API reports the account's setup for the identity, whichever API
        // recorded it: an ENTRY for the identity it was asked about (the map is required
        // and every SDK indexes it by the requested identity), signing on, and the tokens
        // the owner was told to publish.
        let classic = v1_dkim(&router, case, row.domain).await;
        let Some(classic) = classic else {
            panic!(
                "case {case:?}: expected GetIdentityDkimAttributes to carry an entry for {:?}",
                row.domain
            );
        };
        assert_eq!(
            (classic.signing_enabled, &classic.tokens),
            (true, &tokens),
            "case {case:?}: expected the account's setup {tokens:?} with signing on, got \
             {classic:?}"
        );
        // A setup nobody has published the records for yet is reported as waiting on
        // them, in each API's own spelling of that state.
        assert_eq!(
            classic.status, "Pending",
            "case {case:?}: expected the fresh setup reported Pending, got {classic:?}"
        );

        // And so does the v2 API, one identity at a time.
        let read = rest(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{}", row.domain),
            None,
        )
        .await;
        let modern = rest_dkim(&read);
        assert_eq!(
            (modern.signing_enabled, &modern.tokens),
            (true, &tokens),
            "case {case:?}: expected GetEmailIdentity to report the account's setup {tokens:?}, \
             got {modern:?}"
        );
        assert!(
            states_agree(&classic.status, &modern.status),
            "case {case:?}: expected both APIs to report one state, got {:?} and {:?}",
            classic.status,
            modern.status
        );

        // The third API over the same identities reports it too.
        let read = rest(
            &router,
            case,
            "GET",
            &format!("/v1/email/identities/{}", row.domain),
            None,
        )
        .await;
        let pinpoint = rest_dkim(&read);
        assert_eq!(
            (pinpoint.signing_enabled, &pinpoint.tokens),
            (true, &tokens),
            "case {case:?}: expected the Pinpoint Email API to report the account's setup \
             {tokens:?}, got {pinpoint:?}"
        );
        assert!(
            states_agree(&classic.status, &pinpoint.status),
            "case {case:?}: expected every API to report one state, got {:?} and {:?}",
            classic.status,
            pinpoint.status
        );

        // Asking the classic API to set Easy DKIM up for a domain the account already
        // holds a setup for hands back THAT setup: the records a caller has published
        // stay the ones every API reports.
        let answered = v1(
            &router,
            case,
            "VerifyDomainDkim",
            &[("Domain", row.domain.to_string())],
        )
        .await;
        let again: Vec<String> = block_of(&answered, "DkimTokens")
            .map(|list| members_of(list).into_iter().map(str::to_string).collect())
            .unwrap_or_default();
        assert_eq!(
            again, tokens,
            "case {case:?}: expected VerifyDomainDkim to hand back the account's tokens \
             {tokens:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn an_address_reports_the_setup_of_the_domain_that_covers_it() {
    // "A set of DKIM tokens that represent the identity. If the identity is an email
    // address, the tokens represent the domain of that address."
    let router = router();
    let case = "an address under a domain the v2 API created";
    let domain = "covered.example.com";
    let created = rest(
        &router,
        case,
        "POST",
        "/v2/email/identities",
        Some(json!({"EmailIdentity": domain})),
    )
    .await;
    let tokens = rest_dkim(&created).tokens;

    // The rows differ in how the address sits under the setup: a mailbox of the domain
    // itself, and mailboxes of subdomains one and two labels below it.
    let addresses = [
        ("a mailbox of the domain", "sender@covered.example.com"),
        (
            "a mailbox of a subdomain of it",
            "sender@mail.covered.example.com",
        ),
        (
            "a mailbox of a deeper subdomain of it",
            "sender@smtp.mail.covered.example.com",
        ),
    ];
    for (row, address) in addresses {
        v1(
            &router,
            row,
            "VerifyEmailIdentity",
            &[("EmailAddress", address.to_string())],
        )
        .await;
        let reported = v1_dkim(&router, row, address).await;
        let Some(reported) = reported else {
            panic!("case {row:?}: expected GetIdentityDkimAttributes to carry an entry for {address:?}");
        };
        assert_eq!(
            reported.tokens, tokens,
            "case {row:?}: expected the covering domain's tokens {tokens:?}, got {reported:?}"
        );
    }

    // An identity this account does not hold at all is ABSENT from the map, so a caller
    // can tell "no Easy DKIM" from "not an identity of this account" — while the map
    // element itself is always served, since the model makes it required.
    let answered = v1(
        &router,
        "an identity the account does not hold",
        "GetIdentityDkimAttributes",
        &[("Identities.member.1", "stranger.example.org".to_string())],
    )
    .await;
    let map = block_of(&answered, "DkimAttributes");
    assert_eq!(
        map.map(|map| entries_of(map).len()),
        Some(0),
        "case \"an identity the account does not hold\": expected the required map with no \
         entry, got {answered}"
    );
}
