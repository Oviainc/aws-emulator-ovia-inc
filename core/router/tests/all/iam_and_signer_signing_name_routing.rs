// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for two SigV4 signing names that more than one emulated
//! service crate claims: `iam` and `signer`.
//!
//! The rule under test: a request is served by exactly one emulated service, the one
//! whose own endpoint it addresses. Where several service crates share a SigV4 signing
//! name — the router resolves a request's routing key from the credential scope first,
//! so each of them sees the SAME key — each one claims only the requests addressed to
//! it: those naming its own endpoint prefix, and those inside its own URI path space
//! (for a REST service the segments its model binds; for an RPC/awsQuery service the
//! root path it posts every operation to). Claiming on the signing name alone makes
//! every claimant claim every request, and the router then answers `500
//! AmbiguousService` — which is not an AWS error code at all — for all of it, including
//! each service's own operations.
//!
//! `shared_signing_name_routing.rs` pins the same rule for the `s3`/`s3-control` and
//! `iam`/`iam-toolbox` pairs over a hand-mounted router; this suite assembles the WHOLE
//! router instead, and adds the host-style routing signal and the `signer` pair.
//!
//! Two pairs share a name here:
//!
//! * AWS IAM (`awsQuery`, endpoint `iam`, every operation a `POST /`) and IAM Toolbox
//!   (`restJson1`, endpoint `iam-toolbox`, `GET /authorization-details/{authorizationId}`),
//!   both `sigv4Name` `iam`;
//! * AWS Signer (`restJson1`, endpoint `signer`) and the Signer data plane (`restJson1`,
//!   endpoint `data-signer`, `GET /revocations`), both `sigv4Name` `signer`. Signer's own
//!   model binds `GetRevocationStatus` to `@endpoint(hostPrefix: "data-")`, i.e. to the
//!   data-plane endpoint the second crate also emulates: the control plane holds the
//!   revocation records that answer it, so it claims the shared signing name whole
//!   (both hosts), and the data-plane crate claims the key of its OWN endpoint, which
//!   only a request carrying no credential scope resolves to.
//!
//! The rows below span what the rule generalises over: both signing names, both wire
//! protocols (an XML `awsQuery` service and three `restJson1` ones), both routing signals
//! (a SigV4 credential scope and a host-style endpoint), several operations per service,
//! and — in the second table — requests that name an endpoint prefix OUTSIDE its path
//! space, which must reach nobody rather than being swallowed by the peer.
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

/// One request a row sends.
struct Call {
    /// What this row is, for the assertion messages.
    case: &'static str,
    method: &'static str,
    /// Path (and query) only; the host is supplied separately.
    uri: String,
    /// The `Host` header — the host-style routing signal, read when a request carries
    /// no SigV4 credential scope.
    host: String,
    /// The signing name a SigV4 credential scope names, or `None` for an unsigned
    /// (anonymous) request that must route by host alone.
    signing_name: Option<&'static str>,
    content_type: Option<&'static str>,
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

/// An `awsQuery` request body: the `Action` form field the query protocol routes on,
/// plus IAM's API version.
fn query_body(action: &str) -> String {
    format!("Action={action}&Version=2010-05-08")
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

/// A signed `awsQuery` IAM call: `POST /` with the `Action` in the form body.
fn iam_call(case: &'static str, action: &str) -> Call {
    Call {
        case,
        method: "POST",
        uri: "/".to_string(),
        host: format!("iam.{REGION}.amazonaws.com"),
        signing_name: Some("iam"),
        content_type: Some("application/x-www-form-urlencoded"),
        body: query_body(action),
    }
}

/// The full, valid query string of a `GetRevocationStatus` request. Every one of the
/// five members is `@required` + `@httpQuery`
/// (`smithy_query(query="operation",operation="GetRevocationStatus")`).
fn revocations_query() -> String {
    let profile = format!(
        "arn:aws:signer:{REGION}:000000000000:/signing-profiles/RouterProfile_1/AAAAAAAAAA"
    );
    let job = format!("arn:aws:signer:{REGION}:000000000000:/signing-jobs/abcdef0123456789");
    // An ISO 8601 `date-time`, the rendering an `@httpQuery`-bound timestamp carries by
    // default, and a 64-hex-character certificate hash.
    format!(
        "?signatureTimestamp=2023-11-14T22:13:20Z&platformId=AWSLambda-SHA384-ECDSA\
         &profileVersionArn={profile}&jobArn={job}\
         &certificateHashes=0000000000000000000000000000000000000000000000000000000000000001"
    )
}

/// The rows: one request each, in order, against one router over one engine (so the
/// signing-profile read-back sees the write the earlier row made).
fn rows() -> Vec<(Call, Expect)> {
    let profile = "RouterProfile_1";
    vec![
        // ---- the `iam` signing name -------------------------------------------------
        //
        // IAM's operations are all still scaffolded, so their status is not pinned: what
        // this row asserts is that exactly one service claimed the request and answered
        // in IAM's own XML envelope — never a JSON `__type` body an XML-protocol client
        // cannot parse.
        (
            iam_call("iam CreateRole, signed", "CreateRole"),
            Expect {
                status: None,
                code: None,
                xml: true,
                contains: &[],
            },
        ),
        // A second, unrelated action: routing is not keyed to one operation.
        (
            iam_call("iam ListRoles, signed", "ListRoles"),
            Expect {
                status: None,
                code: None,
                xml: true,
                contains: &[],
            },
        ),
        // The same IAM traffic with no credential scope at all, addressed at IAM's
        // endpoint host: the host-style signal resolves the same key.
        (
            Call {
                case: "iam GetUser, unsigned and host-addressed",
                method: "POST",
                uri: "/".to_string(),
                host: format!("iam.{REGION}.amazonaws.com"),
                signing_name: None,
                content_type: Some("application/x-www-form-urlencoded"),
                body: query_body("GetUser"),
            },
            Expect {
                status: None,
                code: None,
                xml: true,
                contains: &[],
            },
        ),
        // IAM Toolbox's own path space, signed with the SHARED signing name: its
        // implemented read answers its own modeled 404 for an id nothing was stored
        // under, which only the owning service can produce.
        (
            Call {
                case: "iam-toolbox authorization details, signed as iam",
                method: "GET",
                // `authorizationId` is `@pattern ^[0123456789abcdefghijklmnopqrstuvwxyz]+$`.
                uri: "/authorization-details/abc123".to_string(),
                host: format!("iam.{REGION}.amazonaws.com"),
                signing_name: Some("iam"),
                content_type: None,
                body: String::new(),
            },
            Expect {
                status: Some(404),
                code: Some("ResourceNotFoundException"),
                xml: false,
                contains: &[],
            },
        ),
        // The same read addressed at IAM Toolbox's OWN endpoint host, unsigned.
        (
            Call {
                case: "iam-toolbox authorization details, host-addressed",
                method: "GET",
                uri: "/authorization-details/abc123".to_string(),
                host: format!("iam-toolbox.{REGION}.amazonaws.com"),
                signing_name: None,
                content_type: None,
                body: String::new(),
            },
            Expect {
                status: Some(404),
                code: Some("ResourceNotFoundException"),
                xml: false,
                contains: &[],
            },
        ),
        // ---- the `signer` signing name ----------------------------------------------
        //
        // AWS Signer's control plane: a write, then the read that proves it was stored
        // by the service that claimed it.
        (
            Call {
                case: "signer PutSigningProfile",
                method: "PUT",
                uri: format!("/signing-profiles/{profile}"),
                host: format!("signer.{REGION}.amazonaws.com"),
                signing_name: Some("signer"),
                content_type: Some("application/json"),
                body: r#"{"platformId":"AWSLambda-SHA384-ECDSA"}"#.to_string(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["profileVersionArn"],
            },
        ),
        (
            Call {
                case: "signer GetSigningProfile reads the profile back",
                method: "GET",
                uri: format!("/signing-profiles/{profile}"),
                host: format!("signer.{REGION}.amazonaws.com"),
                signing_name: Some("signer"),
                content_type: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["RouterProfile_1", "AWSLambda-SHA384-ECDSA"],
            },
        ),
        (
            Call {
                case: "signer ListSigningProfiles",
                method: "GET",
                uri: "/signing-profiles".to_string(),
                host: format!("signer.{REGION}.amazonaws.com"),
                signing_name: Some("signer"),
                content_type: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["RouterProfile_1"],
            },
        ),
        // The `/revocations` path space, signed with the shared signing name: the
        // control plane claims it (it holds the revocation records that answer it) and
        // its read answers 200 with an empty `revokedEntities` list, nothing having
        // been revoked.
        (
            Call {
                case: "signer GetRevocationStatus, signed as signer",
                method: "GET",
                uri: format!("/revocations{}", revocations_query()),
                host: format!("signer.{REGION}.amazonaws.com"),
                signing_name: Some("signer"),
                content_type: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["revokedEntities"],
            },
        ),
        // The same read addressed at the data plane's OWN endpoint host, unsigned: that
        // key is the data-plane crate's alone, so it is the service that answers.
        (
            Call {
                case: "signer data plane GetRevocationStatus, host-addressed",
                method: "GET",
                uri: format!("/revocations{}", revocations_query()),
                host: format!("data-signer.{REGION}.amazonaws.com"),
                signing_name: None,
                content_type: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["revokedEntities"],
            },
        ),
        // A `/revocations` request that leaves every required query parameter out: the
        // service that claimed it refuses it with its own modeled validation error,
        // which again only the owning service can produce.
        (
            Call {
                case: "signer GetRevocationStatus with no parameters",
                method: "GET",
                uri: "/revocations".to_string(),
                host: format!("signer.{REGION}.amazonaws.com"),
                signing_name: Some("signer"),
                content_type: None,
                body: String::new(),
            },
            Expect {
                status: Some(400),
                code: Some("ValidationException"),
                xml: false,
                contains: &["signatureTimestamp"],
            },
        ),
    ]
}

/// A request that names a peer's endpoint prefix but a path OUTSIDE that peer's model:
/// no service is addressed, so the router answers its own `UnknownService` rather than
/// letting the peer swallow traffic that is not its own.
fn unclaimed_rows() -> Vec<Call> {
    vec![
        Call {
            case: "signer control-plane path at the data-plane endpoint",
            method: "GET",
            uri: "/signing-profiles".to_string(),
            host: format!("data-signer.{REGION}.amazonaws.com"),
            signing_name: None,
            content_type: None,
            body: String::new(),
        },
        Call {
            case: "awsQuery root path at the iam-toolbox endpoint",
            method: "POST",
            uri: "/".to_string(),
            host: format!("iam-toolbox.{REGION}.amazonaws.com"),
            signing_name: None,
            content_type: Some("application/x-www-form-urlencoded"),
            body: query_body("CreateRole"),
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
    let request = builder
        .body(bytes::Bytes::from(call.body.clone()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

#[test]
fn every_shared_signing_name_request_reaches_exactly_one_service() {
    std::thread::Builder::new()
        .name("iam-signer-routing".to_string())
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
                    "case {case:?}: a request addressing no service's own path space must \
                     be answered `UnknownService`, got HTTP {status} {code}: {body}"
                );
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("every `iam` and `signer` request must reach exactly one service");
}
