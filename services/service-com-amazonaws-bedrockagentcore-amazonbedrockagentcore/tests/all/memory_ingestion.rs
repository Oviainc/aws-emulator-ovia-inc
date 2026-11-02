// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `IngestData`, the AgentCore Memory long-term ingestion call.
//!
//! The rule under test: a submit persists the inline content it carries against
//! (memory, actor, session) exactly as sent — payload items in order, metadata and
//! namespace variables included — answers 202 with the session it went into (the one
//! the request named, or a newly minted one when it named none), and makes that
//! session readable afterwards; a replayed `clientToken` answers with the session the
//! first submit used and persists nothing further. Ingested content is not a
//! short-term event, so it never appears in `ListEvents`.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so the wire shape (routes, status codes, `restJson1` bodies, modelled
//! error codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use agentcore::ops::types::{Content, IngestPayloadType, MetadataValue};

use super::harness::label;

/// A memory id matching `MemoryId`'s pattern
/// `^(arn:...:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
const MEMORY: &str = "ingest-memory-0123456789";
/// The same memory addressed by its ARN spelling, which that same pattern admits.
const MEMORY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/ingest-memory-0123456789";

/// The emulator, with request authentication off so a test signs nothing, plus the
/// shared engine so a case can read back what a request persisted.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
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

async fn ingest_data(router: &MultiServiceRouter, memory: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/ingest", label(memory)),
        Some(body),
    )
    .await
}

async fn list_sessions(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    body: Value,
) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/actor/{}/sessions", label(memory), label(actor)),
        Some(body),
    )
    .await
}

async fn list_events(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    session: &str,
    body: Value,
) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!(
            "/memories/{}/actor/{}/sessions/{}",
            label(memory),
            label(actor),
            label(session)
        ),
        Some(body),
    )
    .await
}

async fn create_event(router: &MultiServiceRouter, memory: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/events", label(memory)),
        Some(body),
    )
    .await
}

/// `SessionId`'s modelled pattern, `^[a-zA-Z0-9][a-zA-Z0-9-_]*$`, with its 1..100 length.
fn is_modelled_session_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    (1..=100).contains(&id.len())
        && bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A conversational payload item. `Role`'s values are `ASSISTANT`, `USER`, `TOOL`,
/// `OTHER` (source: smithy_query query="shape" shape="Role").
fn conversational(role: &str, text: &str) -> Value {
    json!({"conversational": {"role": role, "content": {"text": text}}})
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// One stored payload item as a comparable string. `IngestPayloadType`'s members are
/// `conversational` and `json` (source: smithy_query query="shape"
/// shape="IngestPayloadType").
fn describe_payload(item: &IngestPayloadType) -> String {
    match item {
        IngestPayloadType::Conversational(turn) => {
            let Content::Text(text) = &turn.content;
            format!("conversational/{}/{text}", turn.role)
        }
        IngestPayloadType::Json(data) => format!("json/{:?}", data.content),
    }
}

/// What one submit persisted: the payload items it stored (in order), its metadata,
/// the namespace variables extraction was asked to use, and its content instant.
#[derive(Debug, PartialEq)]
struct Stored {
    payload: Vec<String>,
    metadata: BTreeMap<String, String>,
    namespace_variables: BTreeMap<String, String>,
    content_timestamp: f64,
}

/// Everything this emulator holds for (memory, actor, session), in ingestion order.
/// The memory is addressed by its bare id, the key both `MemoryId` spellings
/// normalise to.
fn stored_ingestions(
    engine: &engine::InMemoryEngine,
    memory: &str,
    actor: &str,
    session: &str,
) -> Vec<Stored> {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store
        .session(&agentcore::state::normalize_memory_id(memory), actor, session)
        .map(|held| {
            held.ingestions
                .iter()
                .map(|ingestion| Stored {
                    payload: ingestion.payload.iter().map(describe_payload).collect(),
                    metadata: ingestion
                        .metadata
                        .iter()
                        .flatten()
                        .map(|(key, MetadataValue::stringValue(value))| {
                            (key.clone(), value.clone())
                        })
                        .collect(),
                    namespace_variables: ingestion
                        .extraction_config
                        .iter()
                        .filter_map(|config| config.namespace_variables.as_ref())
                        .flatten()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                    content_timestamp: ingestion.content_timestamp.as_secs_f64(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The session ids `ListSessions` reports for one actor, newest-first.
async fn listed_sessions(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    body: Value,
) -> Vec<String> {
    let (status, page) = list_sessions(router, memory, actor, body).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListSessions, got {status} with {page:?}"
    );
    page["sessionSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["sessionId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the submit table: the case name, the `memoryId` spelling, the
/// `sessionId` the body names (`None` when it names none), the inline payload, the
/// `metadata`, the `extractionConfig` and the content instant.
type SubmitCase = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Value,
    Value,
    Value,
    f64,
);

/// Submitted content is persisted as sent, for every shape the `ContentSource` union
/// admits, and the session it went into is readable afterwards. Rows differ along the
/// payload union member, the role, the payload item count, whether `metadata`,
/// `extractionConfig` and `sessionId` were supplied, the `memoryId` spelling and the
/// content instant.
#[tokio::test]
async fn submitted_content_is_persisted_and_its_session_is_readable() {
    let (router, engine) = emulator();

    let cases: Vec<SubmitCase> = vec![
        (
            "one-conversational-user-turn",
            MEMORY,
            Some("ingest-session-a"),
            json!([conversational("USER", "book me a flight")]),
            Value::Null,
            Value::Null,
            1_700_000_100.0,
        ),
        (
            "two-turns-with-metadata",
            MEMORY,
            Some("ingest-session-b"),
            json!([
                conversational("USER", "what did I order?"),
                conversational("ASSISTANT", "a flight to Seattle")
            ]),
            json!({"channel": {"stringValue": "chat"}, "locale": {"stringValue": "en-US"}}),
            Value::Null,
            1_700_000_200.0,
        ),
        (
            "tool-and-other-roles-with-namespace-variables",
            MEMORY,
            Some("ingest-session-c"),
            json!([
                conversational("TOOL", "lookup returned 2 bookings"),
                conversational("OTHER", "note to self")
            ]),
            Value::Null,
            json!({"namespaceVariables": {"tenant": "acme", "region": "us-east-1"}}),
            1_700_000_300.5,
        ),
        (
            "json-content",
            MEMORY,
            Some("ingest-session-d"),
            json!([{"json": {"content": {"activity": "login", "attempts": 3}}}]),
            json!({"kind": {"stringValue": "structured"}}),
            Value::Null,
            1_700_000_400.0,
        ),
        (
            "mixed-payload-of-three-items",
            MEMORY,
            Some("ingest-session-e"),
            json!([
                conversational("USER", "first"),
                {"json": {"content": ["second", 2]}},
                conversational("ASSISTANT", "third")
            ]),
            Value::Null,
            json!({"namespaceVariables": {"tenant": "globex"}}),
            1_700_000_500.0,
        ),
        (
            "memory-addressed-by-arn-without-a-session-id",
            MEMORY_ARN,
            None,
            json!([conversational("USER", "remember this")]),
            json!({"channel": {"stringValue": "voice"}}),
            json!({"namespaceVariables": {"tenant": "initech"}}),
            1_700_000_600.0,
        ),
    ];

    for (case, memory, session, payload, metadata, extraction, stamp) in cases {
        let mut request = json!({
            "actorId": "actor-1",
            "contentTimestamp": stamp,
            "source": {"inline": {"payload": payload.clone()}},
        });
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        if !metadata.is_null() {
            request["metadata"] = metadata.clone();
        }
        if !extraction.is_null() {
            request["extractionConfig"] = extraction.clone();
        }

        let (status, answered) = ingest_data(&router, memory, request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from IngestData, got {status} with {answered:?}"
        );
        let session_id = answered["sessionId"].as_str().unwrap_or_default().to_string();
        match session {
            Some(named) => assert_eq!(
                session_id, named,
                "case {case:?}: expected the named session back, got {session_id:?}"
            ),
            None => assert!(
                is_modelled_session_id(&session_id),
                "case {case:?}: expected a minted sessionId matching ^[a-zA-Z0-9][a-zA-Z0-9-_]*$, got {session_id:?}"
            ),
        }

        // What the submit persisted, read off the store the handlers share.
        let want = Stored {
            payload: payload
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|item| {
                    if let Some(turn) = item.get("conversational") {
                        format!(
                            "conversational/{}/{}",
                            turn["role"].as_str().unwrap_or_default(),
                            turn["content"]["text"].as_str().unwrap_or_default()
                        )
                    } else {
                        // The stored document's `Debug` is compared against itself
                        // below; here only the member is pinned.
                        "json".to_string()
                    }
                })
                .collect(),
            metadata: metadata
                .as_object()
                .map(|entries| {
                    entries
                        .iter()
                        .map(|(key, value)| {
                            (
                                key.clone(),
                                value["stringValue"].as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            namespace_variables: extraction
                .get("namespaceVariables")
                .and_then(Value::as_object)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default(),
            content_timestamp: stamp,
        };
        let held = stored_ingestions(&engine, memory, "actor-1", &session_id);
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected exactly one stored ingestion for the session, got {held:?}"
        );
        let stored = &held[0];
        // A `json` payload item is compared by its member, since its content is a
        // document the wire carries verbatim.
        let seen_members: Vec<String> = stored
            .payload
            .iter()
            .map(|item| {
                if item.starts_with("json/") {
                    "json".to_string()
                } else {
                    item.clone()
                }
            })
            .collect();
        assert_eq!(
            seen_members, want.payload,
            "case {case:?}: expected the payload items {:?} in order, got {seen_members:?}",
            want.payload
        );
        assert_eq!(
            stored.metadata, want.metadata,
            "case {case:?}: expected metadata {:?}, got {:?}",
            want.metadata, stored.metadata
        );
        assert_eq!(
            stored.namespace_variables, want.namespace_variables,
            "case {case:?}: expected namespace variables {:?}, got {:?}",
            want.namespace_variables, stored.namespace_variables
        );
        assert_eq!(
            stored.content_timestamp, want.content_timestamp,
            "case {case:?}: expected the contentTimestamp sent, got {}",
            stored.content_timestamp
        );

        // Read back through an implemented reader: the session now exists in the
        // memory container, with the content instant as its `createdAt`.
        let (status, page) = list_sessions(&router, memory, "actor-1", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListSessions, got {status} with {page:?}"
        );
        let summary = page["sessionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["sessionId"].as_str() == Some(session_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected ListSessions to report {session_id:?}, got {page:?}")
            });
        assert_eq!(
            summary["createdAt"].as_f64(),
            Some(stamp),
            "case {case:?}: expected createdAt {stamp}, got {:?}",
            summary["createdAt"]
        );

        // Ingested content is not a short-term event: nothing reaches `ListEvents`.
        let (status, events) =
            list_events(&router, memory, "actor-1", &session_id, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListEvents, got {status} with {events:?}"
        );
        assert_eq!(
            events["events"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected no events in a session that only received ingested content, got {:?}",
            events["events"]
        );
    }
}

/// A session the caller did not name is minted per submit: two submits without a
/// `sessionId` land in two distinct sessions, each addressable afterwards.
#[tokio::test]
async fn an_omitted_session_id_is_minted_per_submit() {
    let (router, engine) = emulator();
    let mut minted: Vec<String> = Vec::new();

    for case in ["first", "second", "third"] {
        let (status, answered) = ingest_data(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "contentTimestamp": 1_700_000_000,
                "source": {"inline": {"payload": [conversational("USER", case)]}},
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202, got {status} with {answered:?}"
        );
        let session_id = answered["sessionId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_session_id(&session_id),
            "case {case:?}: expected a minted sessionId, got {session_id:?}"
        );
        assert!(
            !minted.contains(&session_id),
            "case {case:?}: expected a sessionId no earlier submit used, got {session_id:?} after {minted:?}"
        );
        minted.push(session_id);
    }

    let listed = listed_sessions(&router, MEMORY, "actor-1", json!({})).await;
    for session_id in &minted {
        assert!(
            listed.contains(session_id),
            "expected ListSessions to report the minted session {session_id:?}, got {listed:?}"
        );
        assert_eq!(
            stored_ingestions(&engine, MEMORY, "actor-1", session_id).len(),
            1,
            "expected one stored ingestion in {session_id:?}, got {:?}",
            stored_ingestions(&engine, MEMORY, "actor-1", session_id)
        );
    }
}

/// `IngestData` is `@idempotent`: a repeated `clientToken` answers with the session the
/// first submit used and stores nothing further, whatever the replay's other members
/// say, while a different token stores a new record.
#[tokio::test]
async fn a_replayed_client_token_ingests_once() {
    let (router, engine) = emulator();

    // Rows: (case, the token, the sessionId the body names, the text it carries), run
    // in order. The first row of each token is the submit that is kept.
    let submits = [
        ("first-submit", "token-1", Some("ingest-session-a"), "one"),
        ("same-token-same-body", "token-1", Some("ingest-session-a"), "one"),
        ("same-token-other-session", "token-1", Some("ingest-session-z"), "two"),
        ("same-token-no-session", "token-1", None, "three"),
        ("second-token", "token-2", Some("ingest-session-b"), "four"),
        ("third-token-no-session", "token-3", None, "five"),
    ];

    let mut answers: Vec<(&str, String)> = Vec::new();
    for (case, token, session, text) in submits {
        let mut request = json!({
            "actorId": "actor-1",
            "contentTimestamp": 1_700_000_100,
            "clientToken": token,
            "source": {"inline": {"payload": [conversational("USER", text)]}},
        });
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        let (status, answered) = ingest_data(&router, MEMORY, request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202, got {status} with {answered:?}"
        );
        answers.push((
            case,
            answered["sessionId"].as_str().unwrap_or_default().to_string(),
        ));
    }

    let first = answers[0].1.clone();
    for (case, answered) in answers.iter().take(4) {
        assert_eq!(
            *answered, first,
            "case {case:?}: expected the replayed token to answer with {first:?}, got {answered:?}"
        );
    }
    assert_eq!(
        stored_ingestions(&engine, MEMORY, "actor-1", &first).len(),
        1,
        "expected one stored ingestion for a token submitted four times, got {:?}",
        stored_ingestions(&engine, MEMORY, "actor-1", &first)
    );
    assert!(
        stored_ingestions(&engine, MEMORY, "actor-1", "ingest-session-z").is_empty(),
        "expected a replay naming another session to store nothing there, got {:?}",
        stored_ingestions(&engine, MEMORY, "actor-1", "ingest-session-z")
    );

    // A different token is a different submit.
    assert_eq!(
        answers[4].1, "ingest-session-b",
        "expected the second token's own session, got {:?}",
        answers[4].1
    );
    assert_eq!(
        stored_ingestions(&engine, MEMORY, "actor-1", "ingest-session-b").len(),
        1,
        "expected the second token to store its own ingestion"
    );
    assert!(
        answers[5].1 != first && is_modelled_session_id(&answers[5].1),
        "expected a third token with no sessionId to mint its own session, got {:?}",
        answers[5].1
    );

    let listed = listed_sessions(&router, MEMORY, "actor-1", json!({})).await;
    assert!(
        !listed.contains(&"ingest-session-z".to_string()),
        "expected no session for a replay that was ignored, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        3,
        "expected one session per distinct clientToken, got {listed:?}"
    );
}

/// The two memory planes stay apart: a session's ingested content and its events are
/// reported by their own operations, and `ListSessions`' `HAS_EVENTS` filter keeps
/// only the sessions holding events.
#[tokio::test]
async fn ingested_content_and_events_share_a_session_without_mixing() {
    let (router, _engine) = emulator();

    // `ingest-only` receives content alone, `both` receives content and an event,
    // `events-only` receives an event alone.
    for (case, session) in [("ingest-only", "ingest-only"), ("both", "both")] {
        let (status, answered) = ingest_data(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "sessionId": session,
                "contentTimestamp": 1_700_000_100,
                "source": {"inline": {"payload": [conversational("USER", case)]}},
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202, got {status} with {answered:?}"
        );
    }
    for session in ["both", "events-only"] {
        let (status, created) = create_event(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "sessionId": session,
                "eventTimestamp": 1_700_000_900,
                "payload": [conversational("USER", "an event")],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "session {session:?}: expected HTTP 201 from CreateEvent, got {status} with {created:?}"
        );
    }

    for (case, filter, expected) in [
        (
            "unfiltered",
            json!({}),
            vec!["both", "events-only", "ingest-only"],
        ),
        (
            "has-events",
            json!({"filter": {"eventFilter": "HAS_EVENTS"}}),
            vec!["both", "events-only"],
        ),
    ] {
        let mut listed = listed_sessions(&router, MEMORY, "actor-1", filter).await;
        listed.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // The event count per session is unchanged by the ingested content beside it.
    for (session, expected) in [("ingest-only", 0), ("both", 1), ("events-only", 1)] {
        let (status, page) = list_events(&router, MEMORY, "actor-1", session, json!({})).await;
        assert_eq!(
            status, 200,
            "session {session:?}: expected HTTP 200 from ListEvents, got {status} with {page:?}"
        );
        assert_eq!(
            page["events"].as_array().map(Vec::len),
            Some(expected),
            "session {session:?}: expected {expected} event(s), got {:?}",
            page["events"]
        );
    }
}

/// Input outside a modelled constraint is refused with a 400, not accepted and not
/// answered with a 5xx or a 501.
#[tokio::test]
async fn out_of_constraint_input_is_refused_with_400() {
    let (router, _engine) = emulator();

    // Rows: the member each one puts outside its modelled bound. Bounds from
    // smithy_query query="operation" operation="IngestData": `payload` 1..100 items,
    // `metadata` 0..15 entries, `sessionId` 1..100 and `^[a-zA-Z0-9][a-zA-Z0-9-_]*$`,
    // `actorId` 1..255, `namespaceVariables` 1..5 entries.
    let metadata_over: Value = (0..16)
        .map(|index| (format!("key{index}"), json!({"stringValue": "v"})))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let namespace_over: Value = (0..6)
        .map(|index| (format!("key{index}"), json!("value")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let cases: Vec<(&str, Value)> = vec![
        (
            "empty-payload",
            json!({
                "actorId": "actor-1",
                "contentTimestamp": 1_700_000_100,
                "source": {"inline": {"payload": []}},
            }),
        ),
        (
            "payload-over-100-items",
            json!({
                "actorId": "actor-1",
                "contentTimestamp": 1_700_000_100,
                "source": {"inline": {"payload": (0..101)
                    .map(|index| conversational("USER", &format!("turn {index}")))
                    .collect::<Vec<Value>>()},
                },
            }),
        ),
        (
            "metadata-over-15-entries",
            json!({
                "actorId": "actor-1",
                "contentTimestamp": 1_700_000_100,
                "metadata": metadata_over,
                "source": {"inline": {"payload": [conversational("USER", "x")]}},
            }),
        ),
        (
            "namespace-variables-over-5-entries",
            json!({
                "actorId": "actor-1",
                "contentTimestamp": 1_700_000_100,
                "extractionConfig": {"namespaceVariables": namespace_over},
                "source": {"inline": {"payload": [conversational("USER", "x")]}},
            }),
        ),
        (
            "session-id-off-pattern",
            json!({
                "actorId": "actor-1",
                "sessionId": "-nope",
                "contentTimestamp": 1_700_000_100,
                "source": {"inline": {"payload": [conversational("USER", "x")]}},
            }),
        ),
        (
            "session-id-over-100-characters",
            json!({
                "actorId": "actor-1",
                "sessionId": "s".repeat(101),
                "contentTimestamp": 1_700_000_100,
                "source": {"inline": {"payload": [conversational("USER", "x")]}},
            }),
        ),
        (
            "actor-id-empty",
            json!({
                "actorId": "",
                "contentTimestamp": 1_700_000_100,
                "source": {"inline": {"payload": [conversational("USER", "x")]}},
            }),
        ),
        (
            "missing-source",
            json!({
                "actorId": "actor-1",
                "contentTimestamp": 1_700_000_100,
            }),
        ),
        (
            "missing-content-timestamp",
            json!({
                "actorId": "actor-1",
                "source": {"inline": {"payload": [conversational("USER", "x")]}},
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, answered) = ingest_data(&router, MEMORY, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered:?}"
        );
        let code = error_code(&answered);
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {code:?} with {answered:?}"
        );
    }

    // And a payload at the modelled maximum is accepted, so the refusal is the bound
    // and not the size.
    let (status, answered) = ingest_data(
        &router,
        MEMORY,
        json!({
            "actorId": "actor-1",
            "sessionId": "at-the-bound",
            "contentTimestamp": 1_700_000_100,
            "source": {"inline": {"payload": (0..100)
                .map(|index| conversational("USER", &format!("turn {index}")))
                .collect::<Vec<Value>>()},
            },
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "a payload of 100 items is inside the modelled bound: got {status} with {answered:?}"
    );
}

/// The ingestion store has a ceiling: once a session passes its bound the oldest
/// record leaves, so request volume alone cannot grow the store without limit.
#[tokio::test]
async fn the_ingestion_store_evicts_its_oldest_records() {
    use agentcore::state::MAX_INGESTIONS_PER_SESSION;
    let (router, engine) = emulator();

    for index in 0..MAX_INGESTIONS_PER_SESSION + 3 {
        let (status, answered) = ingest_data(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "sessionId": "ring",
                "contentTimestamp": 1_700_000_100,
                "source": {"inline": {"payload": [conversational("USER", &format!("turn {index}"))]}},
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "submit {index}: expected HTTP 202, got {status} with {answered:?}"
        );
    }

    let held = stored_ingestions(&engine, MEMORY, "actor-1", "ring");
    assert_eq!(
        held.len(),
        MAX_INGESTIONS_PER_SESSION,
        "expected the session to hold at most {MAX_INGESTIONS_PER_SESSION} ingestions, got {}",
        held.len()
    );
    assert_eq!(
        held.first().map(|stored| stored.payload.clone()),
        Some(vec!["conversational/USER/turn 3".to_string()]),
        "expected the oldest retained record to be the fourth submitted, got {:?}",
        held.first()
    );
}
