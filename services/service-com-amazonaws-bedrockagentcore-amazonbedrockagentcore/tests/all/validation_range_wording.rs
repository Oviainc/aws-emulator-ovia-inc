// SPDX-License-Identifier: CC0-1.0

//! How this service states a request refused for a value outside a modelled numeric
//! `@range` bound.
//!
//! The rule under test: a violated numeric range names the MEMBER PATH and the BOUND it
//! failed, inside AWS's validation envelope, and NOTHING of the value that was rejected —
//! `1 validation error detected: Value at '<member>' failed to satisfy constraint: Member
//! must have value greater than or equal to <min>` (and `less than or equal to <max>` on
//! the other side). It is a property of the SERVICE, not of one operation, so every
//! operation whose model declares such a bound is stated the same way; a refusal of a class
//! this service states with its value (a `@length` bound) keeps the wording the framework
//! states. The `@pattern` and enum classes, which this service also states without the
//! value, are in `validation_constraint_wording`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// The emulator, with request authentication off so a test signs nothing.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(engine));
    router
}

/// Send one request and return its status and parsed JSON body.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://bedrock-agentcore.us-east-1.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

fn error_message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// The clause a violated lower bound is stated with, inside AWS's envelope for one
/// validation error.
fn below_the_bound(member: &str, min: i64) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have value greater than or equal to {min}"
    )
}

/// The clause a violated upper bound is stated with.
fn above_the_bound(member: &str, max: i64) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have value less than or equal to {max}"
    )
}

/// A value outside a modelled numeric range is refused with a 400 `ValidationException`
/// whose message names the member and the bound and withholds the value — for every
/// operation that declares such a bound, on each side of the range.
#[tokio::test]
async fn a_violated_numeric_range_is_stated_without_the_rejected_value() {
    let router = emulator();

    // Rows differ along the operation, the member, which side of the range was broken and
    // how far outside it the value is. `ListABTests$maxResults` is range 1..100 and
    // `StartCodeInterpreterSession$sessionTimeoutSeconds` is range 1..28800 (source:
    // smithy_query query="operation" operation="ListABTests,StartCodeInterpreterSession").
    let cases: Vec<(&str, &str, &str, Option<Value>, String)> = vec![
        (
            "list-ab-tests-max-results-below-the-range",
            "GET",
            "/ab-tests?maxResults=0",
            None,
            below_the_bound("maxResults", 1),
        ),
        (
            "list-ab-tests-max-results-negative",
            "GET",
            "/ab-tests?maxResults=-7",
            None,
            below_the_bound("maxResults", 1),
        ),
        (
            "list-ab-tests-max-results-above-the-range",
            "GET",
            "/ab-tests?maxResults=101",
            None,
            above_the_bound("maxResults", 100),
        ),
        (
            "list-ab-tests-max-results-far-above-the-range",
            "GET",
            "/ab-tests?maxResults=100000",
            None,
            above_the_bound("maxResults", 100),
        ),
        (
            "start-code-interpreter-session-timeout-below-the-range",
            "PUT",
            "/code-interpreters/aws.codeinterpreter.v1/sessions/start",
            Some(json!({"sessionTimeoutSeconds": 0})),
            below_the_bound("sessionTimeoutSeconds", 1),
        ),
        (
            "start-code-interpreter-session-timeout-above-the-range",
            "PUT",
            "/code-interpreters/aws.codeinterpreter.v1/sessions/start",
            Some(json!({"sessionTimeoutSeconds": 28_801})),
            above_the_bound("sessionTimeoutSeconds", 28_800),
        ),
    ];

    for (case, method, path, body, expected) in cases {
        let (status, refused) = send(&router, method, path, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            expected,
            "case {case:?}: expected the member and the bound and no value, got {refused:?}"
        );
    }
}

/// A refusal of another class keeps the framework's wording, value and all: the rule
/// above is about the classes this service states without a value (a numeric range, a
/// `@pattern`, an enum's value set), so it must not quietly strip the value from a
/// `@length` refusal — which still names the value the framework echoes there.
#[tokio::test]
async fn a_refusal_of_another_class_still_states_its_value() {
    let router = emulator();

    // `MemoryRecordId` is `@length` 40..50 and `^mem-[a-zA-Z0-9-_]*$`, so a short id that
    // satisfies the expression is a `@length` refusal rather than a pattern one.
    let (status, refused) = send(
        &router,
        "DELETE",
        "/memories/demo-memory-0123456789/memoryRecords/mem-short",
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for an id shorter than its length minimum, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused:?}"
    );
    let message = error_message(&refused);
    assert_eq!(
        message,
        "1 validation error detected: Value 'mem-short' at 'memoryRecordId' failed to satisfy \
         constraint: Member must have length greater than or equal to 40",
        "expected a length refusal to echo the value it rejected, got {message:?}"
    );
}
