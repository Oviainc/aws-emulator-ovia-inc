// SPDX-License-Identifier: CC0-1.0

//! ONE set of email identities behind the account's email APIs.
//!
//! The rule under test: an email identity belongs to the ACCOUNT, not to the API that
//! recorded it. Amazon SES publishes two APIs over one identity set — the classic
//! `awsQuery` surface (`VerifyEmailIdentity`, `VerifyDomainIdentity`,
//! `GetIdentityVerificationAttributes`, `ListIdentities`, `DeleteIdentity`) and the v2
//! REST surface (`CreateEmailIdentity`, `GetEmailIdentity`, `ListEmailIdentities`,
//! `DeleteEmailIdentity`) — and the Amazon Pinpoint Email API is a third over the same
//! set, so an identity recorded through any one of them is listed and reported by ALL,
//! in each API's own spelling of the verification status (the classic
//! `Pending`/`Success` for v2's `PENDING`/`SUCCESS`), a domain identity's TXT
//! `VerificationToken` is reported through the API that models one, and a delete served
//! on any surface removes the identity from all of them.
//!
//! The rows differ along what the rule generalises over: which surface RECORDED the
//! identity, which one DELETES it, the class of identity (an apex domain, a subdomain,
//! an address), and how the request spelled the domain (a DNS name is compared without
//! regard to case).
//!
//! This suite needs the services over ONE engine, with the other members of the `ses`
//! signing-name family mounted beside them — the same assembly
//! `core/router/src/lib.rs` performs — because the hand-off being asserted exists only
//! when one engine is shared, and the claim that resolves each request to one service
//! exists only when the peers are there to compete for it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, element, form_encode, members_of, opt_element};

/// The Region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// SES v1's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-12-01).
const SES_V1_API_VERSION: &str = "2010-12-01";

/// The decoded length of a domain identity's TXT verification token, as the account's
/// shared identity registry mints it (`service_common::ses_identities`).
const VERIFICATION_TOKEN_BYTES: usize = 32;

/// The two routing refusals the router answers itself, which no row here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none
/// did). Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// Which API recorded an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The classic `awsQuery` API.
    V1,
    /// The v2 REST API.
    V2,
    /// The Amazon Pinpoint Email API, the third over the same identity set.
    PinpointEmail,
}

/// One row: an identity, the surface that records it, and the surface that deletes it.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The identity as the recording request spells it.
    identity: &'static str,
    /// The API that records it.
    recorded_on: Surface,
    /// The API that deletes it, which is the other one for every row here: a delete
    /// served on one surface must remove the identity from both.
    deleted_on: Surface,
    /// The classic API's `IdentityType` for it, which the v2 API spells in
    /// SCREAMING_CASE.
    identity_type_v1: &'static str,
}

/// The rows, spanning both directions of the hand-off and the identity forms.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a domain created with the v2 API",
            identity: "v2-apex.example.com",
            recorded_on: Surface::V2,
            deleted_on: Surface::V1,
            identity_type_v1: "Domain",
        },
        Row {
            case: "a subdomain created with the v2 API",
            identity: "mail.v2-sub.example.org",
            recorded_on: Surface::V2,
            deleted_on: Surface::V1,
            identity_type_v1: "Domain",
        },
        Row {
            case: "an address created with the v2 API",
            identity: "sender@v2-address.example.net",
            recorded_on: Surface::V2,
            deleted_on: Surface::V1,
            identity_type_v1: "EmailAddress",
        },
        Row {
            case: "a mixed-case domain created with the v2 API",
            identity: "V2-MiXeD.Example.COM",
            recorded_on: Surface::V2,
            deleted_on: Surface::V1,
            identity_type_v1: "Domain",
        },
        Row {
            case: "a domain verified with the classic API",
            identity: "v1-apex.example.com",
            recorded_on: Surface::V1,
            deleted_on: Surface::V2,
            identity_type_v1: "Domain",
        },
        Row {
            case: "a subdomain verified with the classic API",
            identity: "mail.v1-sub.example.org",
            recorded_on: Surface::V1,
            deleted_on: Surface::V2,
            identity_type_v1: "Domain",
        },
        Row {
            case: "an address verified with the classic API",
            identity: "sender@v1-address.example.net",
            recorded_on: Surface::V1,
            deleted_on: Surface::V2,
            identity_type_v1: "EmailAddress",
        },
        Row {
            case: "a domain created with the Pinpoint Email API",
            identity: "pinpoint-apex.example.com",
            recorded_on: Surface::PinpointEmail,
            deleted_on: Surface::V1,
            identity_type_v1: "Domain",
        },
        Row {
            case: "an address created with the Pinpoint Email API",
            identity: "sender@pinpoint-address.example.net",
            recorded_on: Surface::PinpointEmail,
            deleted_on: Surface::V2,
            identity_type_v1: "EmailAddress",
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
/// signal the router resolves a routing key from. The signature itself is never
/// checked.
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

/// Send one request, refusing at the door the two answers the ROUTER itself produces:
/// a request that reached nobody, or one several services claimed.
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
    let answered = send(router, case, &format!("SES v1 {action}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SES v1 {action} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered
}

/// One v2-API call: `restJson1` under the `ses` credential scope, in the `/v2/email/…`
/// path space.
async fn v2(
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
    send(router, case, &format!("SESv2 {method} {path}"), request).await
}

/// [`v2`] where the call must succeed.
async fn v2_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let answered = v2(router, case, method, path, body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SESv2 {method} {path} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.json
}

/// The classic API's report for ONE identity, as
/// `(VerificationStatus, VerificationToken)`; `None` when the account holds no such
/// identity at all, which is what the absence of a map entry means.
async fn v1_verification(
    router: &MultiServiceRouter,
    case: &str,
    identity: &str,
) -> Option<(String, Option<String>)> {
    let answered = v1(
        router,
        case,
        "GetIdentityVerificationAttributes",
        &[("Identities.member.1", identity.to_string())],
    )
    .await;
    let attributes = block_of(&answered.text, "VerificationAttributes")?;
    let entry = block_of(attributes, "value")?;
    Some((
        opt_element(entry, "VerificationStatus").unwrap_or_default(),
        opt_element(entry, "VerificationToken"),
    ))
}

/// Every identity the classic API lists, following its cursor to the end.
async fn v1_identities(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut fields: Vec<(&str, String)> = vec![("MaxItems", "2".to_string())];
        if let Some(token) = &token {
            fields.push(("NextToken", token.clone()));
        }
        let answered = v1(router, case, "ListIdentities", &fields).await;
        let page = opt_element(&answered.text, "Identities").unwrap_or_default();
        seen.extend(
            members_of(&page)
                .into_iter()
                .map(|member| member.to_string()),
        );
        token = opt_element(&answered.text, "NextToken");
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() < 1_000,
            "case {case:?}: ListIdentities did not terminate, saw {seen:?}"
        );
    }
    seen
}

/// Every identity the v2 API lists, on the method and URI every AWS SDK sends
/// `ListEmailIdentities` on (`GET /v2/email/identities`), as
/// `(IdentityName, IdentityType, VerificationStatus)`.
async fn v2_identities(router: &MultiServiceRouter, case: &str) -> Vec<(String, String, String)> {
    let listed = v2_ok(router, case, "GET", "/v2/email/identities", None).await;
    listed
        .get("EmailIdentities")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|info| {
            (
                text_of(info, "IdentityName"),
                text_of(info, "IdentityType"),
                text_of(info, "VerificationStatus"),
            )
        })
        .collect()
}

/// One Pinpoint Email call: `restJson1` under the `ses` credential scope, in that API's
/// own `/v1/email/…` path space. The call must succeed.
async fn pinpoint(
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
    let answered = send(
        router,
        case,
        &format!("Pinpoint Email {method} {path}"),
        request,
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: Pinpoint Email {method} {path} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.json
}

/// Every identity the Pinpoint Email API lists, as `(IdentityName, IdentityType)`.
async fn pinpoint_identities(router: &MultiServiceRouter, case: &str) -> Vec<(String, String)> {
    let listed = pinpoint(router, case, "GET", "/v1/email/identities", None).await;
    listed
        .get("EmailIdentities")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|info| (text_of(info, "IdentityName"), text_of(info, "IdentityType")))
        .collect()
}

/// A JSON object's string member, or the empty string.
fn text_of(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether the two APIs' spellings name the same verification state.
///
/// The classic API spells the state in TitleCase and the v2 API in SCREAMING_CASE, so
/// the two reports agree exactly when one is the other's spelling.
fn states_agree(classic: &str, v2: &str) -> bool {
    let expected = match classic {
        "Pending" => "PENDING",
        "Success" => "SUCCESS",
        "Failed" => "FAILED",
        "TemporaryFailure" => "TEMPORARY_FAILURE",
        "NotStarted" => "NOT_STARTED",
        _ => return false,
    };
    expected == v2
}

/// Whether two identity spellings name the same identity: the domain part of an
/// identity is a DNS name, which is compared without regard to case, while the local
/// part of an address is not.
fn same_identity(left: &str, right: &str) -> bool {
    fn key(identity: &str) -> String {
        match identity.rsplit_once('@') {
            Some((local, domain)) => format!("{local}@{}", domain.to_ascii_lowercase()),
            None => identity.to_ascii_lowercase(),
        }
    }
    key(left) == key(right)
}

#[tokio::test]
async fn an_identity_recorded_through_either_api_is_reported_by_both() {
    let router = router();

    // Record every row, each through its own surface.
    for row in rows() {
        let case = row.case;
        match (row.recorded_on, row.identity_type_v1) {
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
            (Surface::PinpointEmail, _) => {
                pinpoint(
                    &router,
                    case,
                    "POST",
                    "/v1/email/identities",
                    Some(json!({"EmailIdentity": row.identity})),
                )
                .await;
            }
            (Surface::V1, "Domain") => {
                let answered = v1(
                    &router,
                    case,
                    "VerifyDomainIdentity",
                    &[("Domain", row.identity.to_string())],
                )
                .await;
                let token = element(&answered.text, "VerificationToken");
                assert_eq!(
                    ::aws_smithy_types::base64::decode(&token)
                        .map(|bytes| bytes.len())
                        .unwrap_or_default(),
                    VERIFICATION_TOKEN_BYTES,
                    "case {case:?}: expected a token decoding to {VERIFICATION_TOKEN_BYTES} \
                     bytes, got {token:?}"
                );
            }
            (Surface::V1, _) => {
                v1(
                    &router,
                    case,
                    "VerifyEmailIdentity",
                    &[("EmailAddress", row.identity.to_string())],
                )
                .await;
            }
        }
    }

    // Both APIs now report every one of them, whichever surface recorded it.
    let classic_list = v1_identities(&router, "the whole account").await;
    let v2_list = v2_identities(&router, "the whole account").await;
    let pinpoint_list = pinpoint_identities(&router, "the whole account").await;
    for row in rows() {
        let case = row.case;
        assert!(
            classic_list
                .iter()
                .any(|listed| same_identity(listed, row.identity)),
            "case {case:?}: expected ListIdentities to report {:?}, got {classic_list:?}",
            row.identity
        );
        let listed = v2_list
            .iter()
            .find(|(name, _, _)| same_identity(name, row.identity));
        let Some((_, identity_type, v2_state)) = listed else {
            panic!(
                "case {case:?}: expected ListEmailIdentities to report {:?}, got {v2_list:?}",
                row.identity
            );
        };
        let expected_type = match row.identity_type_v1 {
            "Domain" => "DOMAIN",
            _ => "EMAIL_ADDRESS",
        };
        assert_eq!(
            identity_type, expected_type,
            "case {case:?}: expected IdentityType {expected_type}, got {identity_type:?}"
        );
        // And so does the third API over the same identity set.
        let pinpoint_listed = pinpoint_list
            .iter()
            .find(|(name, _)| same_identity(name, row.identity));
        let Some((_, pinpoint_type)) = pinpoint_listed else {
            panic!(
                "case {case:?}: expected the Pinpoint Email listing to report {:?}, got \
                 {pinpoint_list:?}",
                row.identity
            );
        };
        assert_eq!(
            pinpoint_type, expected_type,
            "case {case:?}: expected IdentityType {expected_type}, got {pinpoint_type:?}"
        );

        // The classic API reports the identity's verification, in its own spelling of
        // the state the account holds it in, and a DOMAIN identity's TXT token — which
        // only that API models — whichever surface recorded the identity.
        let reported = v1_verification(&router, case, row.identity).await;
        let Some((classic_state, token)) = reported else {
            panic!(
                "case {case:?}: expected GetIdentityVerificationAttributes to report {:?}",
                row.identity
            );
        };
        assert!(
            states_agree(&classic_state, v2_state),
            "case {case:?}: expected both APIs to report one verification state, got \
             {classic_state:?} and {v2_state:?}"
        );
        match row.identity_type_v1 {
            "Domain" => {
                let token = token.unwrap_or_default();
                let decoded = ::aws_smithy_types::base64::decode(&token)
                    .map(|bytes| bytes.len())
                    .unwrap_or_default();
                assert_eq!(
                    decoded, VERIFICATION_TOKEN_BYTES,
                    "case {case:?}: expected a TXT token decoding to \
                     {VERIFICATION_TOKEN_BYTES} bytes, got {token:?}"
                );
                // Stored, not minted per read: the value the owner publishes stays the
                // value the API reports.
                let again = v1_verification(&router, case, row.identity)
                    .await
                    .and_then(|(_, token)| token);
                assert_eq!(
                    again.as_deref(),
                    Some(token.as_str()),
                    "case {case:?}: expected the stored token {token:?} on the next read, got \
                     {again:?}"
                );
            }
            _ => assert!(
                token.is_none(),
                "case {case:?}: an address identity carries no VerificationToken, got {token:?}"
            ),
        }

        // And the v2 API reports it one identity at a time too.
        let read = v2_ok(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{}", row.identity),
            None,
        )
        .await;
        assert!(
            states_agree(&classic_state, &text_of(&read, "VerificationStatus")),
            "case {case:?}: expected GetEmailIdentity to report the state the account holds, \
             got {read}"
        );
    }

    // A page size the classic API serves is served over the identities the v2 API
    // created, too: the listing pages one account's identity set, not one API's.
    let answered = v1(
        &router,
        "a one-identity page",
        "ListIdentities",
        &[("MaxItems", "1".to_string())],
    )
    .await;
    let page = opt_element(&answered.text, "Identities").unwrap_or_default();
    assert_eq!(
        members_of(&page).len(),
        1,
        "case a one-identity page: expected exactly one identity, got {page}"
    );

    // A delete served on either surface removes the identity from BOTH.
    for row in rows() {
        let case = row.case;
        match row.deleted_on {
            Surface::V1 => {
                v1(
                    &router,
                    case,
                    "DeleteIdentity",
                    &[("Identity", row.identity.to_string())],
                )
                .await;
            }
            Surface::V2 => {
                v2_ok(
                    &router,
                    case,
                    "DELETE",
                    &format!("/v2/email/identities/{}", row.identity),
                    None,
                )
                .await;
            }
            Surface::PinpointEmail => {
                pinpoint(
                    &router,
                    case,
                    "DELETE",
                    &format!("/v1/email/identities/{}", row.identity),
                    None,
                )
                .await;
            }
        }

        let reported = v1_verification(&router, case, row.identity).await;
        assert!(
            reported.is_none(),
            "case {case:?}: expected the classic API to report no entry for a deleted \
             identity, got {reported:?}"
        );
        let classic_list = v1_identities(&router, case).await;
        assert!(
            !classic_list
                .iter()
                .any(|listed| same_identity(listed, row.identity)),
            "case {case:?}: expected ListIdentities to drop the deleted identity, got \
             {classic_list:?}"
        );
        let v2_list = v2_identities(&router, case).await;
        assert!(
            !v2_list
                .iter()
                .any(|(name, _, _)| same_identity(name, row.identity)),
            "case {case:?}: expected ListEmailIdentities to drop the deleted identity, got \
             {v2_list:?}"
        );
        let pinpoint_list = pinpoint_identities(&router, case).await;
        assert!(
            !pinpoint_list
                .iter()
                .any(|(name, _)| same_identity(name, row.identity)),
            "case {case:?}: expected the Pinpoint Email listing to drop the deleted identity, \
             got {pinpoint_list:?}"
        );
        let read = v2(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{}", row.identity),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected GetEmailIdentity to answer 404 for a deleted identity, \
             got HTTP {} with {}",
            read.status, read.text
        );
    }
}
