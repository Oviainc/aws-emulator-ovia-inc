// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the two refusals a workspace id can draw, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes.
//!
//! THE RULES UNDER TEST:
//!
//! 1. **A well-formed id that names no workspace** is 404 `ResourceNotFoundException`, and
//!    the message is composed from the request and the caller's account — the workspace id
//!    that was looked up and the account it was looked up in — as
//!    `Workspace with id=<workspaceId> of account=<accountId> does not exist.` The same
//!    rule holds for every operation that resolves a `workspaceId`, because it is the one
//!    lookup they all make.
//! 2. **A value outside `WorkspaceId`'s own `@pattern`** is 400 `ValidationException`, and
//!    the message is produced from the member's modelled constraint —
//!    `The <member> should satisfy the pattern <pattern>.` — naming the member and the
//!    pattern and echoing NO part of the rejected value, which is what keeps a caller's
//!    own string out of the answer.
//!
//! The tables span what the rules generalise over: the operation (every implemented one
//! that takes a `workspaceId` in its URI, across all four HTTP methods this service uses),
//! the id (several well-formed ids nothing minted, including the published bounds of the
//! pattern's hex range) and the WAY a value fails the pattern (prefix, length either side,
//! character class, emptiness, and a value carrying URI punctuation).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `WorkspaceId`'s pattern, from `smithy_query(query="operation",
/// operation="DescribeWorkspace")` — `workspaceId` is `@required`, `@httpLabel` and
/// `@pattern("^g-[0-9a-f]{10}$")`. Spelled out here rather than imported, so the sentence
/// the service states is checked against the MODEL's pattern and not against whatever the
/// implementation happens to carry.
const WORKSPACE_ID_PATTERN: &str = "^g-[0-9a-f]{10}$";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
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
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Every implemented operation that carries a `workspaceId` in its request URI: the name,
/// the method, the URI under `/workspaces/<id>`, and a request body the operation would
/// otherwise accept.
///
/// Source: the operations' own `@http` bindings
/// (`smithy_query(query="digest")`), less the four this emulator still answers 501 for
/// (`AssociateLicense`, `DisassociateLicense`, `ListPermissions`, `UpdatePermissions`),
/// which refuse before they resolve anything.
fn workspace_operations() -> Vec<(&'static str, &'static str, &'static str, Option<Value>)> {
    vec![
        ("DescribeWorkspace", "GET", "", None),
        ("DeleteWorkspace", "DELETE", "", None),
        (
            "UpdateWorkspace",
            "PUT",
            "",
            Some(json!({"workspaceDescription": "unreachable"})),
        ),
        (
            "DescribeWorkspaceConfiguration",
            "GET",
            "/configuration",
            None,
        ),
        (
            "UpdateWorkspaceConfiguration",
            "PUT",
            "/configuration",
            Some(json!({"configuration": "{}"})),
        ),
        (
            "DescribeWorkspaceAuthentication",
            "GET",
            "/authentication",
            None,
        ),
        (
            "UpdateWorkspaceAuthentication",
            "POST",
            "/authentication",
            Some(json!({"authenticationProviders": ["AWS_SSO"]})),
        ),
        ("ListWorkspaceServiceAccounts", "GET", "/serviceaccounts", None),
        (
            "CreateWorkspaceServiceAccount",
            "POST",
            "/serviceaccounts",
            Some(json!({"name": "unreachable", "grafanaRole": "ADMIN"})),
        ),
        (
            "CreateWorkspaceApiKey",
            "POST",
            "/apikeys",
            Some(json!({"keyName": "unreachable", "keyRole": "ADMIN", "secondsToLive": 600})),
        ),
        ("DeleteWorkspaceApiKey", "DELETE", "/apikeys/unreachable", None),
        (
            "DeleteWorkspaceServiceAccount",
            "DELETE",
            "/serviceaccounts/1",
            None,
        ),
        (
            "ListWorkspaceServiceAccountTokens",
            "GET",
            "/serviceaccounts/1/tokens",
            None,
        ),
        (
            "CreateWorkspaceServiceAccountToken",
            "POST",
            "/serviceaccounts/1/tokens",
            Some(json!({"name": "unreachable", "secondsToLive": 600})),
        ),
        (
            "DeleteWorkspaceServiceAccountToken",
            "DELETE",
            "/serviceaccounts/1/tokens/1",
            None,
        ),
    ]
}

/// THE RULE: an id that satisfies the model's pattern but names no workspace is refused
/// with the sentence built from that id and the account it was looked up in — the same
/// sentence from every operation that resolves a `workspaceId`.
#[tokio::test]
async fn an_unknown_workspace_id_is_refused_with_the_id_and_the_account() {
    // Ids the pattern admits that no workspace carries: the two ends of the hex range the
    // pattern publishes, and ordinary values in between.
    let unknown_ids = [
        "g-0000000000",
        "g-ffffffffff",
        "g-0123456789",
        "g-abcdef0123",
    ];
    let account = ::engine::emulator_account_id();

    let router = emulator();
    for id in unknown_ids {
        let expected = format!("Workspace with id={id} of account={account} does not exist.");
        for (operation, method, suffix, body) in workspace_operations() {
            let case = format!("{operation} with {id}");
            let answer = call(
                &router,
                method,
                &format!("/workspaces/{id}{suffix}"),
                body.clone(),
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.body["resourceId"].as_str().unwrap_or_default(),
                id,
                "case {case:?}: the refusal must name the id it could not find, got {:?}",
                answer.body
            );
        }
    }
}

/// THE RULE: a `workspaceId` outside the modelled pattern is refused with the member's own
/// constraint stated in this service's form — naming the member and the pattern, and
/// echoing no part of the value sent.
#[tokio::test]
async fn a_workspace_id_outside_the_pattern_is_refused_with_the_patterns_own_sentence() {
    // (case, the value sent) — the ways a value fails `^g-[0-9a-f]{10}$`.
    let malformed: [(&str, &str); 8] = [
        ("no g- prefix at all", "not-a-valid-workspace-id"),
        ("the prefix but no digits", "g-"),
        ("one hex digit too few", "g-012345678"),
        ("one hex digit too many", "g-0123456789a"),
        ("hex digits out of range", "g-0123456xyz"),
        ("upper-case hex digits, which the pattern does not admit", "g-ABCDEF0123"),
        ("a different prefix", "w-0123456789"),
        ("punctuation inside the id", "g-01234.6789"),
    ];
    let expected = format!("The workspaceId should satisfy the pattern {WORKSPACE_ID_PATTERN}.");

    let router = emulator();
    for (case, id) in malformed {
        for (operation, method, suffix, body) in workspace_operations() {
            let case = format!("{operation} with {case}");
            let answer = call(
                &router,
                method,
                &format!("/workspaces/{id}{suffix}"),
                body.clone(),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
            assert!(
                // A value that is itself a fragment of the sentence (the `g-` prefix the
                // pattern spells) is no evidence either way; every other one would be.
                expected.contains(id) || !answer.message().contains(id),
                "case {case:?}: the refusal must not echo the rejected value, got {:?}",
                answer.message()
            );
        }
    }
}
