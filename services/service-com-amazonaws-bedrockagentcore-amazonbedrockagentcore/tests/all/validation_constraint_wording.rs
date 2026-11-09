// SPDX-License-Identifier: CC0-1.0

//! How this service states a request refused because a value is outside a modelled
//! `@pattern` or outside an enumerated type's value set.
//!
//! THE RULE: such a refusal is a 400 `ValidationException` whose message names the MEMBER
//! PATH and the REQUIREMENT inside AWS's validation envelope and quotes NO submitted
//! value — a `@pattern` requirement stated without the anchors a whole-value expression
//! wraps it in, and an enumerated type's value set stated in the order this service
//! publishes that set. It is a property of the SERVICE, not of one operation or one
//! member, so every constrained member reached by any binding (a URI label, a body member,
//! a member inside a list element, a member inside a map entry's value) is stated the same
//! way.
//!
//! A constraint the request boundary cannot report — a MAP KEY, and a member inside a map
//! entry's value — is applied by the handler against the SAME model constraints, so the
//! over-permissive accept the boundary leaves is closed with the same sentence rather than
//! with a second voice.
//!
//! The rows span what the rule generalises over: the operation, the member, the binding the
//! value arrived in, the depth of the member path, the constraint class, and which
//! character of a value puts it outside its expression.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the routes, status codes and `restJson1` error bodies are exercised and not
//! just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

use super::harness::{create_memory, label};

/// The error code every refusal under test carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A memory id matching `MemoryId`'s pattern, created in the shared inventory so a
/// refusal under test is the request's own and not a missing container.
const MEMORY: &str = "demo-memory-0123456789";

/// The actor and session a valid `CreateEvent` under test writes into.
const ACTOR: &str = "actor-1";
const SESSION: &str = "sess1";

/// `MemoryRecordId`'s pattern and `EventId`'s, `SessionId`'s, `ActorId`'s and
/// `MetadataKey`/`MetadataValue$stringValue`'s, as the model declares them MINUS the
/// `^`/`$` anchors (source: smithy_query query="shape" shape="MemoryRecordId,EventId,
/// SessionId,ActorId,MetadataKey,MetadataValue").
const MEMORY_RECORD_ID_PATTERN: &str = "mem-[a-zA-Z0-9-_]*";
const EVENT_ID_PATTERN: &str = "[0-9]+#[a-fA-F0-9]+";
const SESSION_ID_PATTERN: &str = "[a-zA-Z0-9][a-zA-Z0-9-_]*";
const ACTOR_ID_PATTERN: &str = "[a-zA-Z0-9][a-zA-Z0-9-_/]*(?::[a-zA-Z0-9-_/]+)*[a-zA-Z0-9-_/]*";
const METADATA_PATTERN: &str = r"[a-zA-Z0-9\s._:/=+@-]*";

/// The values AWS lists for `Role` when it refuses a value outside that enum, and the
/// values it lists for `ExtractionMode`, which it states as the model declares them
/// (source: smithy_query query="shape" shape="Role,ExtractionMode").
const STATED_ROLE_VALUES: [&str; 4] = ["USER", "OTHER", "TOOL", "ASSISTANT"];
const EXTRACTION_MODE_VALUES: [&str; 1] = ["SKIP"];

/// The emulator, with request authentication off so a test signs nothing.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    create_memory(&engine, MEMORY);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(engine));
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
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

fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// The whole message a value outside its member's `@pattern` is refused with.
fn pattern_refusal(member: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {pattern}"
    )
}

/// The whole message a value outside an enumerated type's value set is refused with.
fn enum_refusal(member: &str, values: &[&str]) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        values.join(", ")
    )
}

/// `POST /memories/{memoryId}/events` with `body`.
async fn create_event(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/events", label(MEMORY)),
        Some(body),
    )
    .await
}

/// One conversational payload entry the model accepts.
fn conversational(role: &str, text: &str) -> Value {
    json!({"conversational": {"role": role, "content": {"text": text}}})
}

/// The smallest `CreateEvent` body this service accepts, with `extra` merged over it.
fn event_body(extra: Value) -> Value {
    let mut body = json!({
        "actorId": ACTOR,
        "sessionId": SESSION,
        "eventTimestamp": 1_700_000_000,
        "payload": [conversational("USER", "hello")],
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// One row of the table below: the request to send, the whole message it must be refused
/// with, and the value the request submitted — which the message must not echo.
struct Case {
    name: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    expected: String,
    rejected: String,
}

/// Every value outside a modelled `@pattern` or outside an enum's value set is refused
/// with the member path, the requirement and no submitted value — whichever operation,
/// member, binding and depth it arrived at.
#[tokio::test]
async fn a_value_outside_its_pattern_or_enum_is_stated_without_the_rejected_value() {
    let router = emulator();

    let long_x = "x".repeat(44);
    let hashes = format!("mem-{}", "#".repeat(40));
    let cases: Vec<Case> = vec![
        Case {
            name: "delete-memory-record-id-outside-its-pattern",
            method: "DELETE",
            path: format!(
                "/memories/{}/memoryRecords/{}",
                label(MEMORY),
                label(&long_x)
            ),
            body: None,
            expected: pattern_refusal("memoryRecordId", MEMORY_RECORD_ID_PATTERN),
            rejected: long_x.clone(),
        },
        Case {
            name: "delete-memory-record-id-with-a-character-outside-the-expression",
            method: "DELETE",
            path: format!(
                "/memories/{}/memoryRecords/{}",
                label(MEMORY),
                label(&hashes)
            ),
            body: None,
            expected: pattern_refusal("memoryRecordId", MEMORY_RECORD_ID_PATTERN),
            rejected: hashes.clone(),
        },
        Case {
            name: "get-event-id-outside-its-pattern",
            method: "GET",
            path: format!(
                "/memories/{}/actor/{}/sessions/{}/events/{}",
                label(MEMORY),
                label(ACTOR),
                label(SESSION),
                label("not-a-valid-event-id")
            ),
            body: None,
            expected: pattern_refusal("eventId", EVENT_ID_PATTERN),
            rejected: "not-a-valid-event-id".to_string(),
        },
        Case {
            name: "get-event-id-with-a-non-hex-discriminator",
            method: "GET",
            path: format!(
                "/memories/{}/actor/{}/sessions/{}/events/{}",
                label(MEMORY),
                label(ACTOR),
                label(SESSION),
                label("1700000000#xyz")
            ),
            body: None,
            expected: pattern_refusal("eventId", EVENT_ID_PATTERN),
            rejected: "1700000000#xyz".to_string(),
        },
        Case {
            name: "get-event-session-id-outside-its-pattern",
            method: "GET",
            path: format!(
                "/memories/{}/actor/{}/sessions/{}/events/{}",
                label(MEMORY),
                label(ACTOR),
                label("_leading-underscore"),
                label("1700000000#a1b2")
            ),
            body: None,
            expected: pattern_refusal("sessionId", SESSION_ID_PATTERN),
            rejected: "_leading-underscore".to_string(),
        },
        Case {
            name: "create-event-actor-id-outside-its-pattern",
            method: "POST",
            path: format!("/memories/{}/events", label(MEMORY)),
            body: Some(event_body(json!({"actorId": "-leading-dash"}))),
            expected: pattern_refusal("actorId", ACTOR_ID_PATTERN),
            rejected: "-leading-dash".to_string(),
        },
        Case {
            name: "create-event-metadata-string-value-outside-its-pattern",
            method: "POST",
            path: format!("/memories/{}/events", label(MEMORY)),
            body: Some(event_body(
                json!({"metadata": {"channel": {"stringValue": "web#chat"}}}),
            )),
            expected: pattern_refusal("metadata.channel.member.stringValue", METADATA_PATTERN),
            rejected: "web#chat".to_string(),
        },
        Case {
            name: "create-event-metadata-string-value-outside-its-pattern-non-ascii",
            method: "POST",
            path: format!("/memories/{}/events", label(MEMORY)),
            body: Some(event_body(
                json!({"metadata": {"locale": {"stringValue": "trag\u{fc}dien"}}}),
            )),
            expected: pattern_refusal("metadata.locale.member.stringValue", METADATA_PATTERN),
            rejected: "trag\u{fc}dien".to_string(),
        },
        Case {
            name: "create-event-role-outside-the-enum",
            method: "POST",
            path: format!("/memories/{}/events", label(MEMORY)),
            body: Some(event_body(
                json!({"payload": [conversational("NOT_A_VALID_ROLE", "hello")]}),
            )),
            expected: enum_refusal("payload.1.member.conversational.role", &STATED_ROLE_VALUES),
            rejected: "NOT_A_VALID_ROLE".to_string(),
        },
        Case {
            name: "create-event-role-outside-the-enum-at-the-second-payload-entry",
            method: "POST",
            path: format!("/memories/{}/events", label(MEMORY)),
            body: Some(event_body(json!({"payload": [
                conversational("USER", "hello"),
                conversational("assistant", "hi"),
            ]}))),
            expected: enum_refusal("payload.2.member.conversational.role", &STATED_ROLE_VALUES),
            rejected: "assistant".to_string(),
        },
        Case {
            name: "create-event-extraction-mode-outside-the-enum",
            method: "POST",
            path: format!("/memories/{}/events", label(MEMORY)),
            body: Some(event_body(json!({"extractionMode": "ARCHIVE"}))),
            expected: enum_refusal("extractionMode", &EXTRACTION_MODE_VALUES),
            rejected: "ARCHIVE".to_string(),
        },
    ];

    for row in cases {
        let Case {
            name: case,
            method,
            path,
            body,
            expected,
            rejected,
        } = row;
        let (status, refused) = send(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message(&refused)
        );
        assert!(
            !message(&refused).contains(&rejected),
            "case {case:?}: the message must not echo the rejected value {rejected:?}, got {:?}",
            message(&refused)
        );
    }
}

/// A metadata entry outside the constraints `MetadataMap` declares is refused BEFORE
/// anything is stored: the event the call would have created does not exist afterwards,
/// and the session it would have opened was never opened.
#[tokio::test]
async fn metadata_outside_its_constraints_creates_nothing() {
    let router = emulator();

    for (case, metadata) in [
        (
            "a string value outside its pattern",
            json!({"channel": {"stringValue": "web#chat"}}),
        ),
        (
            "a key outside its pattern",
            json!({"chan#nel": {"stringValue": "web"}}),
        ),
        (
            "a string value past its length maximum",
            json!({"channel": {"stringValue": "w".repeat(257)}}),
        ),
    ] {
        let (status, refused) = create_event(&router, event_body(json!({"metadata": metadata}))).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {refused:?}"
        );

        // Nothing was stored: the session the refused call named holds no event at all.
        let (status, listed) = send(
            &router,
            "POST",
            &format!(
                "/memories/{}/actor/{}/sessions/{}",
                label(MEMORY),
                label(ACTOR),
                label(SESSION)
            ),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the listing to be served, got {status} with {listed:?}"
        );
        assert_eq!(
            listed["events"],
            json!([]),
            "case {case:?}: a refused CreateEvent must store nothing, got {listed:?}"
        );
    }

    // The same request WITHOUT the offending metadata is accepted, so the refusals above
    // are about the metadata and not about the rest of the request.
    let (status, created) = create_event(
        &router,
        event_body(json!({"metadata": {"channel": {"stringValue": "web-chat"}}})),
    )
    .await;
    assert_eq!(
        status, 201,
        "metadata inside its constraints must be accepted, got {status} with {created:?}"
    );
    assert_eq!(
        created["event"]["metadata"]["channel"]["stringValue"].as_str(),
        Some("web-chat"),
        "the accepted metadata must be reported back, got {created:?}"
    );
}

/// The whole metadata requirement set a KEY has to satisfy, and the length requirement a
/// `stringValue` has to satisfy, are stated as the framework states any requirement.
#[tokio::test]
async fn a_metadata_key_and_an_overlong_string_value_state_their_requirements() {
    let router = emulator();

    let (status, refused) = create_event(
        &router,
        event_body(json!({"metadata": {"chan#nel": {"stringValue": "web"}}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "a key outside its pattern is refused, got {status} with {refused:?}"
    );
    let expected = format!(
        "1 validation error detected: Value at 'metadata' failed to satisfy constraint: \
         Map keys must satisfy constraint: [Member must have length less than or equal to 128, \
         Member must have length greater than or equal to 1, \
         Member must satisfy regular expression pattern: {METADATA_PATTERN}]"
    );
    assert_eq!(
        message(&refused),
        expected,
        "expected message {expected:?}, got {:?}",
        message(&refused)
    );

    let (status, refused) = create_event(
        &router,
        event_body(json!({"metadata": {"channel": {"stringValue": "w".repeat(257)}}})),
    )
    .await;
    assert_eq!(
        status, 400,
        "a string value past its length maximum is refused, got {status} with {refused:?}"
    );
    let clause = "at 'metadata.channel.member.stringValue' failed to satisfy constraint: \
                  Member must have length less than or equal to 256";
    assert!(
        message(&refused).contains(clause),
        "expected a clause carrying {clause:?}, got {:?}",
        message(&refused)
    );
}
