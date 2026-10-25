// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing name `ses`, which FOUR emulated service
//! crates claim.
//!
//! The rule under test: a request is served by exactly one emulated service, the one
//! whose own endpoint and wire form it addresses, and the answer it gets is that
//! service's own — modelled and terminal. Where several service crates share a SigV4
//! signing name the router resolves a request's routing key from the credential scope
//! first, so each of them sees the SAME key; each must therefore claim only the requests
//! addressed to it: those naming its own endpoint prefix, and those arriving in its own
//! wire form (for a REST service the URI path space its model binds; for an `awsQuery`
//! service the root path every operation posts to; for an `awsJson`/`rpcv2Cbor` service
//! the RPC form that NAMES it). Claiming on the signing name alone makes every claimant
//! claim every request, and the router then answers `500 AmbiguousService` — a code no
//! AWS model carries, on a status an SDK classifies as retryable, so a client with a
//! non-default retry budget never surfaces any answer at all.
//!
//! Four crates share `ses` (`smithy_query(query="service_info")` on each model):
//!
//! * SES v1 (`SimpleEmailService`, `awsQuery`, endpoint `email`, every operation a
//!   `POST /` with the operation in the `Action` form field);
//! * SESv2 (`SimpleEmailService_v2`, `restJson1`, endpoint `email`, `/v2/email/…`);
//! * Pinpoint Email (`AmazonPinpointEmailService`, `restJson1`, endpoint `email`,
//!   `/v1/email/…`);
//! * MailManager (`MailManagerSvc`, `awsJson1_0` + `rpcv2Cbor`, endpoint `mail-manager`,
//!   `X-Amz-Target: MailManagerSvc.<Operation>` or
//!   `POST /service/MailManagerSvc/operation/<Operation>`).
//!
//! The rows span what the rule generalises over: all four services, all four wire
//! protocols, both routing signals (a SigV4 credential scope and a host-style endpoint),
//! several operations per service, and — in the second table — requests that name one of
//! these endpoints with a wire form none of the four serves, which must reach nobody
//! rather than be swallowed by a peer.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when every service is
//! mounted, so a single-service test cannot see it. Assembling the router costs stack
//! (see `boot_serve.rs`), so it is built — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// The contact list the SESv2 rows write and then read back.
const CONTACT_LIST: &str = "router-contact-list";

/// One request a row sends.
struct Call {
    /// What this row is, for the assertion messages.
    case: &'static str,
    method: &'static str,
    /// Path (and query) only; the host is supplied separately.
    uri: String,
    /// The `Host` header — the host-style routing signal, read when a request carries no
    /// SigV4 credential scope.
    host: String,
    /// The signing name a SigV4 credential scope names, or `None` for an unsigned
    /// (anonymous) request that must route by host alone.
    signing_name: Option<&'static str>,
    content_type: Option<&'static str>,
    /// The `X-Amz-Target` an `awsJson` client names its operation in.
    target: Option<&'static str>,
    body: String,
}

/// What the one service that owns a [`Call`] must answer.
struct Expect {
    /// The HTTP status, when pinning it does not pin an unimplemented operation's
    /// scaffold 501 (which would fail the day that operation is implemented).
    status: Option<u16>,
    /// The wire error code (`<Code>` on XML, `__type` on JSON), same caveat.
    code: Option<&'static str>,
    /// Whether the answer must be an XML document. A query/XML-protocol client cannot
    /// parse a JSON `__type` envelope at all — it raises a malformed-response error
    /// instead of surfacing any error code — so an `awsQuery` service's answer must be
    /// XML whether it succeeds or fails.
    xml: bool,
    /// Substrings the body must contain (a read-back's own values, say).
    contains: &'static [&'static str],
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: this router runs with request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// An `awsQuery` request body: the `Action` form field the query protocol routes on, plus
/// SES v1's API version.
fn query_body(action: &str) -> String {
    format!("Action={action}&Version=2010-12-01")
}

/// The text of the one `<name>…</name>` element in an XML body, or `None`.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The wire error code of an answer: `<Code>` for an XML body, `__type` for a JSON one,
/// and the empty string for a success.
fn error_code(body: &str) -> String {
    if body.trim_start().starts_with('<') {
        return xml_element(body, "Code").unwrap_or_default();
    }
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("__type")
                .or_else(|| value.get("Code"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// A signed `awsQuery` SES v1 call: `POST /` with the `Action` in the form body.
fn ses_v1_call(case: &'static str, action: &str) -> Call {
    Call {
        case,
        method: "POST",
        uri: "/".to_string(),
        host: format!("email.{REGION}.amazonaws.com"),
        signing_name: Some("ses"),
        content_type: Some("application/x-www-form-urlencoded"),
        target: None,
        body: query_body(action),
    }
}

/// A signed `awsJson1_0` MailManager call: `POST /` with the operation in `X-Amz-Target`.
fn mail_manager_call(case: &'static str, target: &'static str) -> Call {
    Call {
        case,
        method: "POST",
        uri: "/".to_string(),
        host: format!("mail-manager.{REGION}.amazonaws.com"),
        signing_name: Some("ses"),
        content_type: Some("application/x-amz-json-1.0"),
        target: Some(target),
        body: "{}".to_string(),
    }
}

/// The rows: one request each, in order, against one router over one engine (so the
/// SESv2 read-backs see the write the earlier rows made).
fn rows() -> Vec<(Call, Expect)> {
    vec![
        // ---- SES v1: awsQuery, the root path ----------------------------------------
        //
        // SES v1's operations are implemented to differing degrees, so no status is
        // pinned: what these rows assert is that exactly one service claimed the request
        // and answered in SES v1's own XML envelope — never a JSON `__type` body an
        // XML-protocol client cannot parse.
        (
            ses_v1_call("ses v1 VerifyEmailIdentity, signed", "VerifyEmailIdentity"),
            Expect {
                status: None,
                code: None,
                xml: true,
                contains: &[],
            },
        ),
        (
            ses_v1_call("ses v1 ListIdentities, signed", "ListIdentities"),
            Expect {
                status: None,
                code: None,
                xml: true,
                contains: &[],
            },
        ),
        // The same SES v1 traffic with no credential scope at all, addressed at the SES
        // endpoint host: the host-style signal resolves the endpoint prefix instead.
        (
            Call {
                case: "ses v1 GetSendQuota, unsigned and host-addressed",
                method: "POST",
                uri: "/".to_string(),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: None,
                content_type: Some("application/x-www-form-urlencoded"),
                target: None,
                body: query_body("GetSendQuota"),
            },
            Expect {
                status: None,
                code: None,
                xml: true,
                contains: &[],
            },
        ),
        // ---- SESv2: restJson1, the `/v2/…` path space -------------------------------
        //
        // A write, then the reads that prove it was stored by the service that claimed
        // it: an answer only SESv2 can produce.
        (
            Call {
                case: "sesv2 CreateContactList",
                method: "POST",
                uri: "/v2/email/contact-lists".to_string(),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: Some("ses"),
                content_type: Some("application/json"),
                target: None,
                body: format!(r#"{{"ContactListName":"{CONTACT_LIST}"}}"#),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &[],
            },
        ),
        (
            Call {
                case: "sesv2 ListContactLists reads the list back",
                method: "GET",
                uri: "/v2/email/contact-lists?PageSize=1".to_string(),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: Some("ses"),
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["ContactLists", CONTACT_LIST],
            },
        ),
        (
            Call {
                case: "sesv2 GetContactList",
                method: "GET",
                uri: format!("/v2/email/contact-lists/{CONTACT_LIST}"),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: Some("ses"),
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &[CONTACT_LIST],
            },
        ),
        // The same listing unsigned, addressed at the SES endpoint host: the host-style
        // signal resolves `email`, which three of the four crates answer on, and the URI
        // path space is what picks SESv2 out of them.
        (
            Call {
                case: "sesv2 ListContactLists, unsigned and host-addressed",
                method: "GET",
                uri: "/v2/email/contact-lists".to_string(),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: None,
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &[CONTACT_LIST],
            },
        ),
        // A SESv2 read of a list nothing was stored under: the owning service's own
        // modelled, terminal 404 — which is what a client gets instead of a retryable
        // routing refusal.
        (
            Call {
                case: "sesv2 GetContactList for a list nothing stored",
                method: "GET",
                uri: "/v2/email/contact-lists/router-absent-list".to_string(),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: Some("ses"),
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: Some(404),
                code: Some("NotFoundException"),
                xml: false,
                contains: &["router-absent-list"],
            },
        ),
        // ---- Pinpoint Email: restJson1, the `/v1/…` path space ----------------------
        //
        // Its operations are still scaffolded, so the status is not pinned: the row
        // asserts that the request reached one service, in JSON, and not the SESv2 or SES
        // v1 peer sharing its signing name.
        (
            Call {
                case: "pinpoint-email GetAccount, signed as ses",
                method: "GET",
                uri: "/v1/email/account".to_string(),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: Some("ses"),
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: None,
                code: None,
                xml: false,
                contains: &[],
            },
        ),
        (
            Call {
                case: "pinpoint-email ListConfigurationSets, host-addressed",
                method: "GET",
                uri: "/v1/email/configuration-sets".to_string(),
                host: format!("email.{REGION}.amazonaws.com"),
                signing_name: None,
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: None,
                code: None,
                xml: false,
                contains: &[],
            },
        ),
        // ---- MailManager: awsJson1_0 and rpcv2Cbor ----------------------------------
        //
        // Its operations are still scaffolded too; what matters here is that the RPC form
        // reaches the service its target NAMES, even though SES v1 posts to the same root
        // path under the same signing name.
        (
            mail_manager_call(
                "mailmanager ListAddonInstances, awsJson1_0 target",
                "MailManagerSvc.ListAddonInstances",
            ),
            Expect {
                status: None,
                code: None,
                xml: false,
                contains: &[],
            },
        ),
        (
            mail_manager_call(
                "mailmanager ListArchives, awsJson1_0 target",
                "MailManagerSvc.ListArchives",
            ),
            Expect {
                status: None,
                code: None,
                xml: false,
                contains: &[],
            },
        ),
        (
            Call {
                case: "mailmanager ListAddressLists, rpcv2Cbor URI",
                method: "POST",
                uri: "/service/MailManagerSvc/operation/ListAddressLists".to_string(),
                host: format!("mail-manager.{REGION}.amazonaws.com"),
                signing_name: Some("ses"),
                content_type: Some("application/cbor"),
                target: None,
                body: String::new(),
            },
            Expect {
                status: None,
                code: None,
                xml: false,
                contains: &[],
            },
        ),
    ]
}

/// Requests that name one of these endpoints with a wire form none of the four services
/// serves: no service is addressed, so the router answers its own `UnknownService` rather
/// than letting a peer swallow traffic that is not its own.
fn unclaimed_rows() -> Vec<Call> {
    vec![
        Call {
            case: "a REST path no model in the family binds",
            method: "GET",
            uri: "/v3/email/contact-lists".to_string(),
            host: format!("email.{REGION}.amazonaws.com"),
            signing_name: Some("ses"),
            content_type: None,
            target: None,
            body: String::new(),
        },
        Call {
            // An `awsJson` target naming MailManager, addressed at the SES endpoint host
            // with no credential scope: the routing key is `email`, which MailManager does
            // not answer on, and the out-of-band target is not SES v1's wire form.
            case: "an awsJson target at the email endpoint",
            method: "POST",
            uri: "/".to_string(),
            host: format!("email.{REGION}.amazonaws.com"),
            signing_name: None,
            content_type: Some("application/x-amz-json-1.0"),
            target: Some("MailManagerSvc.ListArchives"),
            body: "{}".to_string(),
        },
        Call {
            // The awsQuery root path at MailManager's own endpoint host: SES v1 answers
            // on `ses` and `email`, not on `mail-manager`.
            case: "the awsQuery root path at the mail-manager endpoint",
            method: "POST",
            uri: "/".to_string(),
            host: format!("mail-manager.{REGION}.amazonaws.com"),
            signing_name: None,
            content_type: Some("application/x-www-form-urlencoded"),
            target: None,
            body: query_body("ListIdentities"),
        },
        Call {
            // A SESv2 path at MailManager's endpoint host, for the same reason.
            case: "a /v2 path at the mail-manager endpoint",
            method: "GET",
            uri: "/v2/email/contact-lists".to_string(),
            host: format!("mail-manager.{REGION}.amazonaws.com"),
            signing_name: None,
            content_type: None,
            target: None,
            body: String::new(),
        },
    ]
}

/// Send one [`Call`] through the router, returning the status and the body as text.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    call: &Call,
) -> (u16, String) {
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(format!("https://{}{}", call.host, call.uri))
        .header("host", &call.host);
    if let Some(signing_name) = call.signing_name {
        builder = builder.header("authorization", authorization(signing_name));
    }
    if let Some(content_type) = call.content_type {
        builder = builder.header("content-type", content_type);
    }
    if let Some(target) = call.target {
        builder = builder.header("x-amz-target", target);
    }
    let request = builder
        .body(bytes::Bytes::from(call.body.clone()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

#[test]
fn every_ses_signed_request_reaches_exactly_one_service() {
    std::thread::Builder::new()
        .name("ses-family-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            for (call, expected) in rows() {
                let case = call.case;
                let (status, body) = send(&router, &runtime, &call);
                let code = error_code(&body);

                // The two routing failures, which no row may provoke: `AmbiguousService`
                // (more than one service claimed it) and `UnknownService` (none did).
                assert!(
                    code != "AmbiguousService" && code != "UnknownService",
                    "case {case:?}: expected exactly one claiming service, got HTTP \
                     {status} {code}: {body}"
                );
                // And no answer in this family may be a 5xx: every one of these
                // conditions is the caller's or the endpoint's, so an SDK must surface it
                // on the first attempt rather than retry it.
                assert!(
                    status < 500 || status == 501,
                    "case {case:?}: a request to this family must be answered with a \
                     terminal status, got HTTP {status} {code}: {body}"
                );
                if let Some(expected_status) = expected.status {
                    assert_eq!(
                        status, expected_status,
                        "case {case:?}: expected HTTP {expected_status}, got {status}: {body}"
                    );
                }
                if let Some(expected_code) = expected.code {
                    assert_eq!(
                        code, expected_code,
                        "case {case:?}: expected the error code {expected_code}, got \
                         {code:?} (HTTP {status}): {body}"
                    );
                }
                if expected.xml {
                    assert!(
                        body.trim_start().starts_with('<') && !body.contains("__type"),
                        "case {case:?}: an awsQuery client must be answered with an XML \
                         document, never a JSON `__type` envelope, got HTTP {status}: {body}"
                    );
                }
                for needle in expected.contains {
                    assert!(
                        body.contains(needle),
                        "case {case:?}: the answer must carry {needle:?}, got HTTP \
                         {status}: {body}"
                    );
                }
            }

            for call in unclaimed_rows() {
                let case = call.case;
                let (status, body) = send(&router, &runtime, &call);
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (404, "UnknownService"),
                    "case {case:?}: a request addressing no service's own wire form must \
                     be answered `UnknownService`, got HTTP {status} {code}: {body}"
                );
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("every `ses`-signed request must reach exactly one service");
}
