// SPDX-License-Identifier: CC0-1.0

//! ONE set of sending-authorization policies, and ONE Easy DKIM sub-state, behind the
//! account's email APIs.
//!
//! Two rules, both about facts that belong to the ACCOUNT's identity rather than to the
//! API that wrote them.
//!
//! The first: the sending-authorization policies attached to an identity are the
//! account's identity's, so a policy attached through any one of the account's email APIs
//! is reported by every API that reports policies — the classic API's name list and the
//! v2 API's name-to-document map are two views of one set — deleting the identity through
//! any API drops them from every view, and recording the same identity name again starts
//! it with none. A client that grants a cross-account send with one API and audits or
//! reconciles the grant with the other must be told the same thing by both.
//!
//! The second: Easy DKIM is started only by a call that ASKS for it. An identity recorded
//! by a call that does not — the classic API's `VerifyDomainIdentity`, which only adds the
//! domain and mints its TXT token, or `VerifyEmailIdentity` — reads back through every API
//! with DKIM not started, no tokens and signing off; only after a call that does ask (the
//! classic `VerifyDomainDkim`, or the v2 `CreateEmailIdentity` of a domain) do they all
//! report the minted token set with signing on. A client that branches on that sub-state
//! to decide whether it still has to publish CNAME records must not be told a setup it
//! never asked for already exists.
//!
//! The rows differ along what the rules generalise over: which API writes the fact, which
//! API reads it, the form of the identity (an apex domain, a subdomain, a mixed-case
//! domain — a DNS name is compared without regard to case — and a mailbox), how many
//! policies the identity carries, and which API deletes it.
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

use crate::query_wire::{block_of, decoded_entries, form_encode, members_of, opt_element};

/// The Region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// SES v1's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-12-01).
const SES_V1_API_VERSION: &str = "2010-12-01";

/// How many CNAME tokens an Easy DKIM setup carries: `DkimAttributes$Tokens` (sesv2) is
/// "a set of unique strings that you use to create a set of CNAME records", and AWS
/// publishes three of them.
const EASY_DKIM_TOKEN_COUNT: usize = 3;

/// The two routing refusals the router answers itself, which no row here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none
/// did). Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// Which API a row writes through, or reads with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// The classic `awsQuery` API (`PutIdentityPolicy`, `VerifyDomainIdentity`).
    Classic,
    /// The v2 REST API (`CreateEmailIdentityPolicy`, `CreateEmailIdentity`).
    V2,
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
/// operation in the `Action` form field. The answer is handed back as it came.
async fn classic_call(
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

/// [`classic_call`] where the call must succeed, answering the XML body.
async fn classic(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    fields: &[(&str, String)],
) -> String {
    let answered = classic_call(router, case, action, fields).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SES v1 {action} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.text
}

/// One REST call — `restJson1` under the `ses` credential scope, in the path space of the
/// API that serves it — answered as it came.
async fn rest_call(
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

/// [`rest_call`] where the call must succeed, answering the parsed body.
async fn rest(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let answered = rest_call(router, case, method, path, body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: {method} {path} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.json
}

/// One sending-authorization policy naming `identity` as the resource it authorizes
/// sending through, written the way a client sends one: a statement with the `Principal`
/// it grants to.
fn policy(identity: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"Authorize\",\
         \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"arn:aws:iam::000000000000:root\"}},\
         \"Action\":\"ses:SendEmail\",\
         \"Resource\":\"arn:aws:ses:{REGION}:000000000000:identity/{identity}\"}}]}}"
    )
}

/// Record `identity` as an identity of the account through `api`, by the call that adds
/// an identity and nothing else.
async fn record_identity(router: &MultiServiceRouter, case: &str, api: Api, identity: &str) {
    match api {
        Api::Classic if identity.contains('@') => {
            classic(
                router,
                case,
                "VerifyEmailIdentity",
                &[("EmailAddress", identity.to_string())],
            )
            .await;
        }
        Api::Classic => {
            classic(
                router,
                case,
                "VerifyDomainIdentity",
                &[("Domain", identity.to_string())],
            )
            .await;
        }
        Api::V2 => {
            rest(
                router,
                case,
                "POST",
                "/v2/email/identities",
                Some(json!({"EmailIdentity": identity})),
            )
            .await;
        }
    }
}

/// Attach the policy `name` to `identity` through `api`.
async fn attach_policy(
    router: &MultiServiceRouter,
    case: &str,
    api: Api,
    identity: &str,
    name: &str,
) {
    match api {
        Api::Classic => {
            classic(
                router,
                case,
                "PutIdentityPolicy",
                &[
                    ("Identity", identity.to_string()),
                    ("PolicyName", name.to_string()),
                    ("Policy", policy(identity)),
                ],
            )
            .await;
        }
        Api::V2 => {
            rest(
                router,
                case,
                "POST",
                &format!("/v2/email/identities/{identity}/policies/{name}"),
                Some(json!({"Policy": policy(identity)})),
            )
            .await;
        }
    }
}

/// The policy NAMES the classic API's `ListIdentityPolicies` reports for `identity`, in
/// the order it reports them.
async fn classic_names(router: &MultiServiceRouter, case: &str, identity: &str) -> Vec<String> {
    let answered = classic(
        router,
        case,
        "ListIdentityPolicies",
        &[("Identity", identity.to_string())],
    )
    .await;
    block_of(&answered, "PolicyNames")
        .map(|list| members_of(list).into_iter().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The `(name, document)` pairs the classic API's `GetIdentityPolicies` reports for the
/// names asked for.
async fn classic_documents(
    router: &MultiServiceRouter,
    case: &str,
    identity: &str,
    names: &[&str],
) -> std::collections::BTreeMap<String, String> {
    // `PolicyNames` is a list, so each name travels as its own indexed form field.
    let asked: Vec<(String, String)> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                format!("PolicyNames.member.{}", index + 1),
                (*name).to_string(),
            )
        })
        .collect();
    let mut fields: Vec<(&str, String)> = vec![("Identity", identity.to_string())];
    fields.extend(
        asked
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone())),
    );
    let answered = classic(router, case, "GetIdentityPolicies", &fields).await;
    block_of(&answered, "Policies")
        .map(decoded_entries)
        .unwrap_or_default()
}

/// The `Policies` map a v2 REST answer carries, as `(name, document)` pairs.
fn rest_policies(reported: &Value) -> std::collections::BTreeMap<String, String> {
    reported
        .get("Policies")
        .and_then(Value::as_object)
        .map(|policies| {
            policies
                .iter()
                .map(|(name, document)| {
                    (
                        name.clone(),
                        document.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the policy rule.
struct PolicyRow {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The identity as the recording request spells it.
    identity: &'static str,
    /// The API that records the identity.
    recorded_on: Api,
    /// The API that attaches the policies.
    attached_on: Api,
    /// The policy names attached, in the order they are attached.
    names: &'static [&'static str],
    /// The API that deletes the identity.
    deleted_on: Api,
}

/// The rows, spanning the writing API, the reading API, the identity's form, how many
/// policies it carries, and the deleting API.
fn policy_rows() -> Vec<PolicyRow> {
    vec![
        PolicyRow {
            case: "one policy attached with the v2 API to a domain it created",
            identity: "v2-made.example.com",
            recorded_on: Api::V2,
            attached_on: Api::V2,
            names: &["CascadePolicy"],
            deleted_on: Api::V2,
        },
        PolicyRow {
            case: "two policies attached with the v2 API to a classic-API domain",
            identity: "v1-made.example.org",
            recorded_on: Api::Classic,
            attached_on: Api::V2,
            names: &["Delegate_One", "Delegate-Two"],
            deleted_on: Api::Classic,
        },
        PolicyRow {
            case: "a policy attached with the classic API to a v2 subdomain",
            identity: "mail.v2-sub.example.net",
            recorded_on: Api::V2,
            attached_on: Api::Classic,
            names: &["Authorize2024"],
            deleted_on: Api::V2,
        },
        PolicyRow {
            case: "a policy attached with the classic API to a mixed-case domain",
            identity: "MiXeD.Example.COM",
            recorded_on: Api::Classic,
            attached_on: Api::Classic,
            names: &["AuthorizeMarketer"],
            deleted_on: Api::Classic,
        },
        PolicyRow {
            case: "a policy attached with the v2 API to a mailbox identity",
            identity: "sender@mailbox.example.com",
            recorded_on: Api::Classic,
            attached_on: Api::V2,
            names: &["AuthorizeMailbox"],
            deleted_on: Api::V2,
        },
    ]
}

/// Delete `identity` through `api`.
async fn delete_identity(router: &MultiServiceRouter, case: &str, api: Api, identity: &str) {
    match api {
        Api::Classic => {
            classic(
                router,
                case,
                "DeleteIdentity",
                &[("Identity", identity.to_string())],
            )
            .await;
        }
        Api::V2 => {
            rest(
                router,
                case,
                "DELETE",
                &format!("/v2/email/identities/{identity}"),
                None,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_policy_attached_through_one_api_is_reported_by_the_others() {
    let router = router();

    for row in policy_rows() {
        let case = row.case;
        let identity = row.identity;
        record_identity(&router, case, row.recorded_on, identity).await;
        for name in row.names {
            attach_policy(&router, case, row.attached_on, identity, name).await;
        }

        // The classic API lists exactly the names attached, whichever API attached them,
        // in one stable (lexicographic) order.
        let mut expected: Vec<String> = row.names.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let listed = classic_names(&router, case, identity).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected ListIdentityPolicies to report {expected:?}, got {listed:?}"
        );

        // And the two APIs that report the DOCUMENTS report the text that was submitted,
        // byte for byte, for every one of those names.
        let document = policy(identity);
        let expected_documents: std::collections::BTreeMap<String, String> = row
            .names
            .iter()
            .map(|name| ((*name).to_string(), document.clone()))
            .collect();
        let classic_docs = classic_documents(&router, case, identity, row.names).await;
        assert_eq!(
            classic_docs, expected_documents,
            "case {case:?}: expected GetIdentityPolicies to report the submitted documents, got \
             {classic_docs:?}"
        );
        let read = rest(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{identity}/policies"),
            None,
        )
        .await;
        let v2_docs = rest_policies(&read);
        assert_eq!(
            v2_docs, expected_documents,
            "case {case:?}: expected GetEmailIdentityPolicies to report the submitted documents, \
             got {v2_docs:?}"
        );
        let read = rest(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{identity}"),
            None,
        )
        .await;
        let on_identity = rest_policies(&read);
        assert_eq!(
            on_identity, expected_documents,
            "case {case:?}: expected GetEmailIdentity to report the identity's policies, got \
             {on_identity:?}"
        );

        // A third API over the same identities has no policy operations at all, so
        // reading the identity through it reports none and disturbs none: the set is
        // still there afterwards.
        rest(
            &router,
            case,
            "GET",
            &format!("/v1/email/identities/{identity}"),
            None,
        )
        .await;
        let listed = classic_names(&router, case, identity).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the policies to survive a read through an API that has \
             none, got {listed:?}"
        );

        // Deleting the identity drops its policies from every view, and recording the
        // same name again does not resurrect them.
        delete_identity(&router, case, row.deleted_on, identity).await;
        let listed = classic_names(&router, case, identity).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected a deleted identity to carry no policy, got {listed:?}"
        );

        record_identity(&router, case, row.recorded_on, identity).await;
        let listed = classic_names(&router, case, identity).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected the re-recorded identity to start with no policy, got \
             {listed:?}"
        );
        let answered = rest_call(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{identity}/policies"),
            None,
        )
        .await;
        for name in row.names {
            assert!(
                !answered.text.contains(name),
                "case {case:?}: expected the re-recorded identity to carry no policy {name:?}, \
                 got {}",
                answered.text
            );
        }
    }
}

/// What one API reports about an identity's Easy DKIM sub-state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dkim {
    /// The state, in the API's own spelling.
    status: String,
    /// The tokens the owner publishes, in the order the API reports them.
    tokens: Vec<String>,
    /// Whether messages from the identity are signed, as the v2 API reports it
    /// (`SigningEnabled`).
    signing_enabled: Option<bool>,
}

/// The classic API's `GetIdentityDkimAttributes` report for `identity`.
async fn classic_dkim(router: &MultiServiceRouter, case: &str, identity: &str) -> Dkim {
    let answered = classic(
        router,
        case,
        "GetIdentityDkimAttributes",
        &[("Identities.member.1", identity.to_string())],
    )
    .await;
    let entry = block_of(&answered, "DkimAttributes")
        .and_then(|map| block_of(map, "value"))
        .unwrap_or_default();
    Dkim {
        status: opt_element(entry, "DkimVerificationStatus").unwrap_or_default(),
        tokens: block_of(entry, "DkimTokens")
            .map(|list| members_of(list).into_iter().map(str::to_string).collect())
            .unwrap_or_default(),
        // `IdentityDkimAttributes$DkimEnabled` documents its own default ("the default
        // value is true"), so it says whether SES would sign rather than whether a setup
        // exists; the state and the tokens are what this rule is about.
        signing_enabled: None,
    }
}

/// A REST API's `GetEmailIdentity` report of the same sub-state, read at `path` — the v2
/// API's identity path, or the Pinpoint Email API's.
async fn rest_dkim(router: &MultiServiceRouter, case: &str, path: &str) -> Dkim {
    let read = rest(router, case, "GET", path, None).await;
    let dkim = read.get("DkimAttributes").cloned().unwrap_or(Value::Null);
    Dkim {
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
        signing_enabled: Some(
            dkim.get("SigningEnabled")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        ),
    }
}

/// The v2 API's identity path for `identity`.
fn v2_identity_path(identity: &str) -> String {
    format!("/v2/email/identities/{identity}")
}

/// The Pinpoint Email API's identity path for `identity`: a third API over the same
/// account identities, which reports the Easy DKIM sub-state as well.
fn pinpoint_identity_path(identity: &str) -> String {
    format!("/v1/email/identities/{identity}")
}

#[tokio::test]
async fn easy_dkim_starts_only_when_a_call_asks_for_it() {
    let router = router();

    // (case, the identity, the API that records it, whether the recording call asks for
    // Easy DKIM). The rows span the calls that add an identity without asking — the
    // classic API's domain and mailbox verifications — against the one create that does
    // ask, over the identity forms the rule covers.
    let rows = [
        (
            "a domain added with the classic VerifyDomainIdentity",
            "v1-domain.example.com",
            Api::Classic,
            false,
        ),
        (
            "a subdomain added with the classic VerifyDomainIdentity",
            "mail.v1-sub.example.org",
            Api::Classic,
            false,
        ),
        (
            "a mixed-case domain added with the classic VerifyDomainIdentity",
            "V1-MiXeD.Example.NET",
            Api::Classic,
            false,
        ),
        (
            "a mailbox verified with the classic VerifyEmailIdentity",
            "sender@v1-mailbox.example.com",
            Api::Classic,
            false,
        ),
        (
            "a domain created with the v2 CreateEmailIdentity",
            "v2-domain.example.com",
            Api::V2,
            true,
        ),
    ];

    for (case, identity, api, asks_for_dkim) in rows {
        record_identity(&router, case, api, identity).await;
        let classic_reported = classic_dkim(&router, case, identity).await;
        let v2_reported = rest_dkim(&router, case, &v2_identity_path(identity)).await;
        let pinpoint_reported =
            rest_dkim(&router, case, &pinpoint_identity_path(identity)).await;

        if !asks_for_dkim {
            // Nothing asked for Easy DKIM, so the process was never initiated for the
            // identity: no tokens to publish, nothing signing, in every API's spelling.
            assert_eq!(
                classic_reported.status, "NotStarted",
                "case {case:?}: expected the classic API to report DKIM not started, got \
                 {classic_reported:?}"
            );
            assert_eq!(
                (v2_reported.status.as_str(), v2_reported.signing_enabled),
                ("NOT_STARTED", Some(false)),
                "case {case:?}: expected the v2 API to report DKIM not started with signing \
                 off, got {v2_reported:?}"
            );
            // The third API over the same identities reports it too, in its own model's
            // spelling — and carries no DKIM member at all for a mailbox identity, since
            // Easy DKIM is set up for a domain.
            let expected_pinpoint_status = if identity.contains('@') {
                ""
            } else {
                "NOT_STARTED"
            };
            assert_eq!(
                (
                    pinpoint_reported.status.as_str(),
                    pinpoint_reported.signing_enabled
                ),
                (expected_pinpoint_status, Some(false)),
                "case {case:?}: expected the Pinpoint Email API to report no DKIM setup under \
                 way, got {pinpoint_reported:?}"
            );
            for (api, reported) in [
                ("classic", &classic_reported),
                ("v2", &v2_reported),
                ("Pinpoint Email", &pinpoint_reported),
            ] {
                assert!(
                    reported.tokens.is_empty(),
                    "case {case:?}: expected the {api} API to report no DKIM token for an \
                     identity nothing set DKIM up for, got {reported:?}"
                );
            }
            continue;
        }

        // The create that DOES ask for Easy DKIM starts it: the token set the owner
        // publishes, signing on, and the search for the records still pending, reported
        // the same way by every API over the account's identities.
        assert_eq!(
            (
                classic_reported.status.as_str(),
                classic_reported.tokens.len()
            ),
            ("Pending", EASY_DKIM_TOKEN_COUNT),
            "case {case:?}: expected the classic API to report a pending setup of \
             {EASY_DKIM_TOKEN_COUNT} tokens, got {classic_reported:?}"
        );
        for (api, reported) in [("v2", &v2_reported), ("Pinpoint Email", &pinpoint_reported)] {
            assert_eq!(
                (
                    reported.status.as_str(),
                    &reported.tokens,
                    reported.signing_enabled
                ),
                ("PENDING", &classic_reported.tokens, Some(true)),
                "case {case:?}: expected the {api} API to report the same pending setup, got \
                 {reported:?}"
            );
        }
    }

    // And the call that asks for Easy DKIM on a domain the classic API added starts it
    // for every API over the account's identities: the tokens it hands out are the ones
    // they all report from then on.
    for (case, identity, _, asks_for_dkim) in rows {
        if asks_for_dkim || identity.contains('@') {
            continue;
        }
        let answered = classic(
            &router,
            case,
            "VerifyDomainDkim",
            &[("Domain", identity.to_string())],
        )
        .await;
        let handed: Vec<String> = block_of(&answered, "DkimTokens")
            .map(|list| members_of(list).into_iter().map(str::to_string).collect())
            .unwrap_or_default();
        assert_eq!(
            handed.len(),
            EASY_DKIM_TOKEN_COUNT,
            "case {case:?}: expected VerifyDomainDkim to hand out {EASY_DKIM_TOKEN_COUNT} \
             tokens, got {handed:?}"
        );
        let classic_reported = classic_dkim(&router, case, identity).await;
        assert_eq!(
            (classic_reported.status.as_str(), &classic_reported.tokens),
            ("Pending", &handed),
            "case {case:?}: expected the classic API to report the setup it just handed out, \
             got {classic_reported:?}"
        );
        let v2_reported = rest_dkim(&router, case, &v2_identity_path(identity)).await;
        let pinpoint_reported = rest_dkim(&router, case, &pinpoint_identity_path(identity)).await;
        for (api, reported) in [("v2", &v2_reported), ("Pinpoint Email", &pinpoint_reported)] {
            assert_eq!(
                (
                    reported.status.as_str(),
                    &reported.tokens,
                    reported.signing_enabled
                ),
                ("PENDING", &handed, Some(true)),
                "case {case:?}: expected the {api} API to report the setup the classic API \
                 started, got {reported:?}"
            );
        }
    }
}
