// SPDX-License-Identifier: CC0-1.0

//! How EVERY paginated read of this service refuses a `nextToken` it never
//! handed out, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpLabel` / `@httpQuery` bindings, deserialization, the handler and
//! serialization — the same path a client takes. The one rule under test:
//!
//! * a paginated read refuses a `nextToken` it did not issue with the modelled
//!   400 `ValidationException` whose `code` names the condition a paginating
//!   client resets on, and the refusal STATES that condition without quoting any
//!   of the caller's token: the token is the service's own opaque cursor, and the
//!   member it arrives on carries no size bound, so an echo would make the error
//!   body as large as the request parameter.
//!
//! The rows differ along what the rule generalises over: which paginated read is
//! called (every one of the four this service binds, and both the path that
//! resolves a policy and the one that finds none for the read whose cursor is a
//! statement id), and the form, alphabet and size of the refused token —
//! including one past the ceiling this crate states for any request value a
//! clause quotes in full, and one a DIFFERENT paginated read of this same
//! service really issued.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite calls through.
const REGION: &str = "us-west-2";

/// A platform whose profiles need no `signingMaterial`, so a seeded profile is
/// one `PutSigningProfile` call.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The error `code` a `nextToken` this service never handed out carries,
/// spelled out here rather than imported so a wrong code in a handler fails
/// this suite.
const ERROR_CODE_INVALID_NEXT_TOKEN: &str = "InvalidNextToken";

/// The modelled error shape such a refusal carries (HTTP 400), as the wire
/// spells it.
const ERROR_SHAPE_VALIDATION: &str = "ValidationException";

/// The sentence the refusal states — the condition, with none of the caller's
/// token in it. Written out here so a handler that quotes the token back, or
/// words the condition differently from its siblings, fails this suite.
const EXPECTED_MESSAGE: &str = "Invalid next token provided";

/// The most bytes the refusal's `message` may carry, whatever the caller sent.
///
/// The rule is that the message is built from the CONDITION and not from the
/// request, so its size cannot depend on the request's; this bound is simply far
/// below the smallest token row that would show an echo (and far below the
/// 16 KiB ceiling this crate states for a request value a clause does quote).
const MAX_MESSAGE_BYTES: usize = 256;

/// How many bytes of a refused token are long enough that finding them in a
/// message means the message was built from the token.
const ECHO_PROBE_BYTES: usize = 4;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `nextToken` this answer carries, or `None` on a final page.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

/// `value` percent-encoded for ONE query value: `nextToken` is an unconstrained
/// `@httpQuery` string, so an SDK escapes everything outside the unreserved set.
/// Spelled out here, the way this crate's other suites spell it, so a test suite
/// takes no dependency of its own.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One paginated read of this service: the operation, what the row varies about
/// it, and the collection path a page of it is asked for at.
struct PaginatedRead {
    operation: &'static str,
    case: &'static str,
    path: String,
}

/// Seed the emulator and answer every paginated read this service binds.
///
/// `ListProfilePermissions` appears twice: its cursor is a statement id of the
/// named profile's policy, so a profile that holds a policy and one that holds
/// none at all reach the refusal by different paths in the handler.
async fn seed(router: &MultiServiceRouter) -> Vec<PaginatedRead> {
    for name in ["Token_Profile_01", "Token_Profile_02"] {
        let put = call(
            router,
            "PUT",
            &format!("/signing-profiles/{name}"),
            Some(json!({ "platformId": PLATFORM_ID })),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "seeding profile {name}: expected 200, got {} {:?}",
            put.status, put.body
        );
    }

    // Only the first profile is granted anything, so only its policy exists.
    let granted = call(
        router,
        "POST",
        "/signing-profiles/Token_Profile_01/permissions",
        Some(json!({
            "statementId": "held",
            "action": "signer:GetSigningProfile",
            "principal": "123456789012",
        })),
    )
    .await;
    assert_eq!(
        granted.status, 200,
        "seeding a permission: expected 200, got {} {:?}",
        granted.status, granted.body
    );

    vec![
        PaginatedRead {
            operation: "ListSigningProfiles",
            case: "ListSigningProfiles",
            path: "/signing-profiles".to_string(),
        },
        PaginatedRead {
            operation: "ListSigningJobs",
            case: "ListSigningJobs",
            path: "/signing-jobs".to_string(),
        },
        PaginatedRead {
            operation: "ListSigningPlatforms",
            case: "ListSigningPlatforms",
            path: "/signing-platforms".to_string(),
        },
        PaginatedRead {
            operation: "ListProfilePermissions",
            case: "ListProfilePermissions on a profile that holds a policy",
            path: "/signing-profiles/Token_Profile_01/permissions".to_string(),
        },
        PaginatedRead {
            operation: "ListProfilePermissions",
            case: "ListProfilePermissions on a profile that holds no policy",
            path: "/signing-profiles/Token_Profile_02/permissions".to_string(),
        },
    ]
}

/// Ask `read` for the page `token` resumes.
async fn page_from(router: &MultiServiceRouter, read: &PaginatedRead, token: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{}?nextToken={}", read.path, encode(token)),
        None,
    )
    .await
}

/// Assert the refusal of `token` by `read` is the modelled one and quotes none
/// of the token.
fn assert_refused_without_echo(read: &PaginatedRead, token_case: &str, token: &str, answer: &Answer) {
    let case = format!("{} refusing {token_case}", read.case);
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        Some(ERROR_CODE_INVALID_NEXT_TOKEN),
        "case {case:?}: expected code {ERROR_CODE_INVALID_NEXT_TOKEN}, got {:?}",
        answer.code()
    );
    assert_eq!(
        answer.shape(),
        ERROR_SHAPE_VALIDATION,
        "case {case:?}: expected the error shape {ERROR_SHAPE_VALIDATION}, got {:?}",
        answer.shape()
    );
    assert_eq!(
        answer.message(),
        EXPECTED_MESSAGE,
        "case {case:?}: expected the message {EXPECTED_MESSAGE:?}, got {:?}",
        answer.message()
    );
    // The whole token, and any leading run of it long enough to identify it: a
    // message built from the condition carries neither.
    assert!(
        !answer.message().contains(token),
        "case {case:?}: expected the refusal to quote none of the token, got {:?}",
        answer.message()
    );
    let probe = &token[..token.len().min(ECHO_PROBE_BYTES)];
    assert!(
        !answer.message().contains(probe),
        "case {case:?}: expected the refusal to carry no part of the token, found {probe:?} in \
         {:?}",
        answer.message()
    );
    assert!(
        answer.message().len() <= MAX_MESSAGE_BYTES,
        "case {case:?}: expected at most {MAX_MESSAGE_BYTES} bytes of message whatever the \
         {} byte token, got {}",
        token.len(),
        answer.message().len()
    );
}

/// Every paginated read refuses a token it never handed out with the modelled
/// 400, and states the condition without quoting the caller's value —
/// whatever the token's form, alphabet or size.
#[tokio::test]
async fn an_unknown_next_token_is_refused_without_echoing_it() {
    let router = emulator();
    let reads = seed(&router).await;

    // Rows differ in the token's form, alphabet and size: a value in no token
    // alphabet at all, one carrying a distinctive marker, one whose characters
    // an SDK escapes, hex digits of the right alphabet but the wrong length,
    // hex digits that decode to no cursor of any listing, and one past the
    // 16 KiB ceiling this crate states for a request value a clause quotes in
    // full — the row an echo turns into an error body as large as the request.
    let oversized = "a".repeat(32 * 1024);
    let token_cases: Vec<(&str, String)> = vec![
        ("a value in no token alphabet", "nope".to_string()),
        (
            "a value carrying a distinctive marker",
            "marker-7f3c-statement".to_string(),
        ),
        (
            "characters an SDK percent-escapes",
            "not a token/../x".to_string(),
        ),
        (
            "the token alphabet at the wrong length",
            "0123456789abcdef".to_string(),
        ),
        (
            "hex digits that decode to no cursor",
            "6162636465666768".repeat(4),
        ),
        ("a token past the stated-value ceiling", oversized),
    ];

    for read in &reads {
        for (token_case, token) in &token_cases {
            let answer = page_from(&router, read, token).await;
            assert_refused_without_echo(read, token_case, token, &answer);
        }
    }
}

/// A token one paginated read really issued is refused by the OTHERS, and that
/// refusal reports nothing of what the cursor carries.
#[tokio::test]
async fn a_token_issued_by_another_paginated_read_is_refused_without_echoing_it() {
    let router = emulator();
    let reads = seed(&router).await;

    // Two real cursors, each from a listing this emulator holds more of than
    // the page asked for, so the page hands a token back.
    let mut issued: Vec<(&str, String)> = Vec::new();
    for (issuer, path) in [
        ("ListSigningProfiles", "/signing-profiles"),
        ("ListSigningPlatforms", "/signing-platforms"),
    ] {
        let first = call(&router, "GET", &format!("{path}?maxResults=1"), None).await;
        assert_eq!(
            first.status, 200,
            "seeding a cursor from {issuer}: expected 200, got {} {:?}",
            first.status, first.body
        );
        let token = first.next_token().unwrap_or_else(|| {
            panic!(
                "seeding a cursor from {issuer}: expected a nextToken, got {:?}",
                first.body
            )
        });
        issued.push((issuer, token));
    }

    for (issuer, token) in &issued {
        for read in &reads {
            // The listing that issued the cursor resumes it: only the OTHER
            // reads refuse it.
            if read.operation == *issuer {
                continue;
            }
            let answer = page_from(&router, read, token).await;
            assert_refused_without_echo(
                read,
                &format!("a cursor {issuer} issued"),
                token,
                &answer,
            );
        }
    }
}
