// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore Memory event lifecycle: `CreateEvent`,
//! `ListSessions`, `ListEvents`, `GetEvent` and `DeleteEvent`.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so the wire shape (routes, status codes, `restJson1` bodies, modelled
//! error codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use super::harness::label;

/// A memory id matching `MemoryId`'s pattern
/// `^(arn:...:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
const MEMORY: &str = "demo-memory-0123456789";
/// The same memory addressed by its ARN spelling, which that same pattern admits.
const MEMORY_ARN: &str = "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/demo-memory-0123456789";

/// The emulator, with request authentication off so a test signs nothing.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService::new(engine),
    );
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

async fn create_event(router: &MultiServiceRouter, memory: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/events", label(memory)),
        Some(body),
    )
    .await
}

async fn get_event(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    session: &str,
    event: &str,
) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!(
            "/memories/{}/actor/{}/sessions/{}/events/{}",
            label(memory),
            label(actor),
            label(session),
            label(event)
        ),
        None,
    )
    .await
}

async fn delete_event(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    session: &str,
    event: &str,
) -> (u16, Value) {
    send(
        router,
        "DELETE",
        &format!(
            "/memories/{}/actor/{}/sessions/{}/events/{}",
            label(memory),
            label(actor),
            label(session),
            label(event)
        ),
        None,
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

/// `EventId`'s modelled pattern, `^[0-9]+#[a-fA-F0-9]+$`, checked without a regex
/// dependency.
fn is_modelled_event_id(id: &str) -> bool {
    match id.split_once('#') {
        Some((millis, suffix)) => {
            !millis.is_empty()
                && millis.bytes().all(|b| b.is_ascii_digit())
                && !suffix.is_empty()
                && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// `SessionId`'s modelled pattern, `^[a-zA-Z0-9][a-zA-Z0-9-_]*$`, with its 1..100 length.
fn is_modelled_session_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    (1..=100).contains(&id.len())
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A conversational payload item. `Role`'s values are `ASSISTANT`, `USER`, `TOOL`,
/// `OTHER` (source: smithy_query shape="Role").
fn conversational(role: &str, text: &str) -> Value {
    json!({"conversational": {"role": role, "content": {"text": text}}})
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A created event is reported from what was stored, for every payload shape the
/// `PayloadType` union models and every `Role` the model lists — and a `GetEvent`
/// afterwards returns byte-identical members.
#[tokio::test]
async fn a_created_event_round_trips_through_get_event() {
    let router = emulator();

    // Rows differ along the payload union member, the role, the payload item count,
    // and whether `metadata` / `branch` were supplied.
    // `PayloadType` members: conversational | blob | json (source: smithy_query
    // shape="PayloadType"); `Role` values as above.
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "conversational-user",
            json!([conversational("USER", "hello")]),
            Value::Null,
            Value::Null,
        ),
        (
            "conversational-assistant-with-metadata",
            json!([conversational("ASSISTANT", "hi there")]),
            json!({"topic": {"stringValue": "greeting"}}),
            Value::Null,
        ),
        (
            "conversational-tool-and-other-two-items",
            json!([
                conversational("TOOL", "tool said this"),
                conversational("OTHER", "and this")
            ]),
            Value::Null,
            Value::Null,
        ),
        (
            "json-payload-with-branch",
            json!([{"json": {"content": {"activity": "login", "attempts": 3}}}]),
            Value::Null,
            json!({"name": "main"}),
        ),
        (
            "blob-payload",
            json!([{"blob": [1, 2, {"nested": true}]}]),
            json!({"kind": {"stringValue": "binary"}}),
            Value::Null,
        ),
    ];

    for (case, payload, metadata, branch) in cases {
        let mut request = json!({
            "actorId": "actor-1",
            "sessionId": case,
            "eventTimestamp": 1_700_000_000,
            "payload": payload,
        });
        if !metadata.is_null() {
            request["metadata"] = metadata.clone();
        }
        if !branch.is_null() {
            request["branch"] = branch.clone();
        }

        let (status, created) = create_event(&router, MEMORY, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateEvent, got {status} with {created:?}"
        );
        let event = &created["event"];
        let event_id = event["eventId"].as_str().unwrap_or_default();
        assert!(
            is_modelled_event_id(event_id),
            "case {case:?}: expected a server-minted eventId matching ^[0-9]+#[a-fA-F0-9]+$, got {event_id:?}"
        );
        assert_eq!(
            event["payload"], payload,
            "case {case:?}: expected the stored payload back unchanged, got {:?}",
            event["payload"]
        );
        assert_eq!(
            event["metadata"],
            if metadata.is_null() {
                Value::Null
            } else {
                metadata.clone()
            },
            "case {case:?}: expected metadata {metadata:?}, got {:?}",
            event["metadata"]
        );
        assert_eq!(
            event["branch"],
            if branch.is_null() {
                Value::Null
            } else {
                branch.clone()
            },
            "case {case:?}: expected branch {branch:?}, got {:?}",
            event["branch"]
        );
        assert_eq!(
            event["memoryId"], MEMORY,
            "case {case:?}: expected memoryId {MEMORY}, got {:?}",
            event["memoryId"]
        );
        assert_eq!(
            event["eventTimestamp"].as_f64(),
            Some(1_700_000_000.0),
            "case {case:?}: expected the eventTimestamp sent, got {:?}",
            event["eventTimestamp"]
        );

        let (status, fetched) = get_event(&router, MEMORY, "actor-1", case, event_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetEvent, got {status} with {fetched:?}"
        );
        assert_eq!(
            fetched["event"], *event,
            "case {case:?}: expected GetEvent to return the created event, got {:?}",
            fetched["event"]
        );
    }
}

/// Every `eventId` is minted by the service: a caller cannot choose one, and two
/// events never share one.
#[tokio::test]
async fn event_ids_are_minted_and_distinct() {
    let router = emulator();
    let mut seen: Vec<String> = Vec::new();

    for case in ["first", "second", "third"] {
        let (status, created) = create_event(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "sessionId": "sess1",
                "eventTimestamp": 1_700_000_000,
                "payload": [conversational("USER", case)],
                // A client-supplied `eventId` is not a modelled input member; a body
                // carrying one must not become the event's id.
                "eventId": "1#deadbeef",
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201, got {status} with {created:?}"
        );
        let id = created["event"]["eventId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            id, "1#deadbeef",
            "case {case:?}: expected a minted eventId, got the one in the request body"
        );
        assert!(
            !seen.contains(&id),
            "case {case:?}: expected a fresh eventId, got the already-issued {id:?}"
        );
        seen.push(id);
    }
}

/// A session the caller did not name is created for it, reported on the event, and
/// addressable afterwards.
#[tokio::test]
async fn an_omitted_session_id_is_generated_and_addressable() {
    let router = emulator();

    for case in ["no-session-1", "no-session-2"] {
        let (status, created) = create_event(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "eventTimestamp": 1_700_000_000,
                "payload": [conversational("USER", case)],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201, got {status} with {created:?}"
        );
        let session_id = created["event"]["sessionId"].as_str().unwrap_or_default();
        assert!(
            is_modelled_session_id(session_id),
            "case {case:?}: expected a generated sessionId matching ^[a-zA-Z0-9][a-zA-Z0-9-_]*$, got {session_id:?}"
        );

        let event_id = created["event"]["eventId"].as_str().unwrap_or_default();
        let (status, fetched) = get_event(&router, MEMORY, "actor-1", session_id, event_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the generated session to address the event, got {status} with {fetched:?}"
        );
    }
}

/// `CreateEvent` is `@idempotent`: a repeated `clientToken` returns the first event and
/// stores no second one, while a different token stores a new event.
#[tokio::test]
async fn a_replayed_client_token_returns_the_same_event() {
    let router = emulator();
    let request = |token: &str, text: &str| {
        json!({
            "actorId": "actor-1",
            "sessionId": "sess1",
            "eventTimestamp": 1_700_000_000,
            "payload": [conversational("USER", text)],
            "clientToken": token,
        })
    };

    // Rows: the number of times one token is replayed. Each row must leave exactly one
    // event behind for that token.
    for (case, replays) in [("token-a", 1_u32), ("token-b", 2), ("token-c", 4)] {
        let (_, first) = create_event(&router, MEMORY, request(case, case)).await;
        let first_id = first["event"]["eventId"].as_str().unwrap_or_default().to_string();
        for replay in 0..replays {
            let (status, again) = create_event(&router, MEMORY, request(case, "a different text")).await;
            assert_eq!(
                status, 201,
                "case {case:?} replay {replay}: expected HTTP 201, got {status} with {again:?}"
            );
            assert_eq!(
                again["event"]["eventId"].as_str(),
                Some(first_id.as_str()),
                "case {case:?} replay {replay}: expected the first eventId back, got {:?}",
                again["event"]["eventId"]
            );
            assert_eq!(
                again["event"]["payload"], first["event"]["payload"],
                "case {case:?} replay {replay}: expected the first event's payload, got {:?}",
                again["event"]["payload"]
            );
        }
    }

    let (_, listed) = list_events(&router, MEMORY, "actor-1", "sess1", json!({})).await;
    let events = listed["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        events.len(),
        3,
        "expected one event per distinct clientToken (3), got {} in {listed:?}",
        events.len()
    );

    // A token has nothing to replay once the event it created is deleted, so it stores
    // a new event rather than reporting the id that is gone.
    let (_, first) = create_event(&router, MEMORY, request("token-d", "first")).await;
    let gone = first["event"]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, _) = delete_event(&router, MEMORY, "actor-1", "sess1", &gone).await;
    assert_eq!(status, 200, "expected HTTP 200 from DeleteEvent, got {status}");
    let (status, after) = create_event(&router, MEMORY, request("token-d", "again")).await;
    assert_eq!(
        status, 201,
        "replay after a delete: expected HTTP 201, got {status} with {after:?}"
    );
    assert_ne!(
        after["event"]["eventId"].as_str(),
        Some(gone.as_str()),
        "replay after a delete: expected a new event, got the deleted id back"
    );
    let (status, still_gone) = get_event(&router, MEMORY, "actor-1", "sess1", &gone).await;
    assert_eq!(
        status, 404,
        "replay after a delete: expected the deleted event to stay gone, got {status} with {still_gone:?}"
    );
}

/// `ListEvents` reports a session's events newest-first by `eventTimestamp`, honours
/// `includePayloads`, and pages deterministically for every `maxResults`.
#[tokio::test]
async fn list_events_is_newest_first_and_pages_over_every_page_size() {
    let router = emulator();
    // Created out of order on purpose, with one fractional-second timestamp, so the
    // ordering cannot come from insertion order alone.
    let timestamps = [
        1_700_000_030.0_f64,
        1_700_000_010.0,
        1_700_000_020.5,
        1_700_000_000.0,
    ];
    for (index, stamp) in timestamps.iter().enumerate() {
        let (status, created) = create_event(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "sessionId": "sess1",
                "eventTimestamp": stamp,
                "payload": [conversational("USER", &format!("event {index}"))],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "event {index}: expected HTTP 201, got {status} with {created:?}"
        );
    }
    let mut newest_first = timestamps;
    newest_first.sort_by(|left, right| right.partial_cmp(left).expect("no NaN timestamps"));

    let (status, listed) = list_events(&router, MEMORY, "actor-1", "sess1", json!({})).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} with {listed:?}");
    let seen: Vec<f64> = listed["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| event["eventTimestamp"].as_f64().unwrap_or_default())
        .collect();
    assert_eq!(
        seen,
        newest_first.to_vec(),
        "expected events newest-first, got {seen:?}"
    );
    assert!(
        listed["nextToken"].is_null(),
        "expected no nextToken on a complete page, got {:?}",
        listed["nextToken"]
    );

    // Every page size walks the same sequence exactly once.
    for page_size in [1_usize, 2, 3, 4, 100] {
        let mut collected: Vec<f64> = Vec::new();
        let mut token = Value::Null;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(value) = token.as_str() {
                request["nextToken"] = json!(value);
            }
            let (status, page) = list_events(&router, MEMORY, "actor-1", "sess1", request).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected HTTP 200, got {status} with {page:?}"
            );
            let events = page["events"].as_array().cloned().unwrap_or_default();
            assert!(
                events.len() <= page_size,
                "page size {page_size}: expected at most {page_size} events, got {}",
                events.len()
            );
            collected.extend(
                events
                    .iter()
                    .map(|event| event["eventTimestamp"].as_f64().unwrap_or_default()),
            );
            token = page["nextToken"].clone();
            pages += 1;
            assert!(
                pages <= 10,
                "page size {page_size}: expected pagination to terminate, got {pages} pages"
            );
            if token.is_null() {
                break;
            }
        }
        assert_eq!(
            collected,
            newest_first.to_vec(),
            "page size {page_size}: expected every event once, newest-first, got {collected:?}"
        );
    }

    // `includePayloads` defaults to true and drops the payloads when false.
    for (case, request, expect_payload) in [
        ("default", json!({}), true),
        ("explicit-true", json!({"includePayloads": true}), true),
        ("false", json!({"includePayloads": false}), false),
    ] {
        let (status, page) = list_events(&router, MEMORY, "actor-1", "sess1", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {page:?}"
        );
        for event in page["events"].as_array().cloned().unwrap_or_default() {
            let carries = !event["payload"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .is_empty();
            assert_eq!(
                carries, expect_payload,
                "case {case:?}: expected payload carried = {expect_payload}, got {:?}",
                event["payload"]
            );
        }
    }
}

/// `ListEvents` narrows by `filter`: by branch (with and without the parent branches)
/// and by event-metadata expressions, for every `OperatorType` the model lists.
#[tokio::test]
async fn list_events_honours_the_modelled_filters() {
    let router = emulator();
    let mut ids: Vec<(String, String)> = Vec::new();
    // A trunk event, a branch forked from it, and a branch forked from that branch.
    for (case, branch, metadata) in [
        ("trunk", Value::Null, json!({"stage": {"stringValue": "intro"}})),
        ("alpha", json!({"name": "alpha"}), json!({"stage": {"stringValue": "middle"}})),
        ("beta", json!({"name": "beta"}), Value::Null),
    ] {
        let mut request = json!({
            "actorId": "actor-1",
            "sessionId": "sess1",
            "eventTimestamp": 1_700_000_000,
            "payload": [conversational("USER", case)],
        });
        if !metadata.is_null() {
            request["metadata"] = metadata;
        }
        if !branch.is_null() {
            let mut branch = branch;
            // `alpha` forks from the trunk event, `beta` forks from `alpha`'s event.
            let root = match case {
                "alpha" => ids.first(),
                _ => ids.get(1),
            };
            if let Some((_, root_id)) = root {
                branch["rootEventId"] = json!(root_id);
            }
            request["branch"] = branch;
        }
        let (status, created) = create_event(&router, MEMORY, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201, got {status} with {created:?}"
        );
        ids.push((
            case.to_string(),
            created["event"]["eventId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }
    let payload_text = |event: &Value| {
        event["payload"][0]["conversational"]["content"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    // Rows: each filter the model can express, and the events it must keep.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "branch-alpha-without-parents",
            json!({"branch": {"name": "alpha", "includeParentBranches": false}}),
            vec!["alpha"],
        ),
        (
            "branch-alpha-with-parents",
            json!({"branch": {"name": "alpha"}}),
            vec!["trunk", "alpha"],
        ),
        (
            "branch-beta-with-parents",
            json!({"branch": {"name": "beta", "includeParentBranches": true}}),
            vec!["trunk", "alpha", "beta"],
        ),
        (
            "branch-unknown",
            json!({"branch": {"name": "gamma", "includeParentBranches": false}}),
            vec![],
        ),
        (
            "metadata-exists",
            json!({"eventMetadata": [{"left": {"metadataKey": "stage"}, "operator": "EXISTS"}]}),
            vec!["trunk", "alpha"],
        ),
        (
            "metadata-not-exists",
            json!({"eventMetadata": [{"left": {"metadataKey": "stage"}, "operator": "NOT_EXISTS"}]}),
            vec!["beta"],
        ),
        (
            "metadata-equals",
            json!({"eventMetadata": [{
                "left": {"metadataKey": "stage"},
                "operator": "EQUALS_TO",
                "right": {"metadataValue": {"stringValue": "middle"}}
            }]}),
            vec!["alpha"],
        ),
        (
            "metadata-equals-no-match",
            json!({"eventMetadata": [{
                "left": {"metadataKey": "stage"},
                "operator": "EQUALS_TO",
                "right": {"metadataValue": {"stringValue": "absent"}}
            }]}),
            vec![],
        ),
        (
            "branch-and-metadata-together",
            json!({
                "branch": {"name": "alpha"},
                "eventMetadata": [{"left": {"metadataKey": "stage"}, "operator": "EXISTS"}]
            }),
            vec!["trunk", "alpha"],
        ),
    ];

    for (case, filter, expected) in cases {
        let (status, page) =
            list_events(&router, MEMORY, "actor-1", "sess1", json!({"filter": filter})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {page:?}"
        );
        let mut seen: Vec<String> = page["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(payload_text)
            .collect();
        seen.sort();
        let mut want: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
        want.sort();
        assert_eq!(
            seen, want,
            "case {case:?}: expected the filter to keep {want:?}, got {seen:?}"
        );
    }

    // `EQUALS_TO` has nothing to compare against without its right operand: the model
    // leaves `right` optional for `EXISTS` / `NOT_EXISTS`, so this is refused here.
    let (status, refused) = list_events(
        &router,
        MEMORY,
        "actor-1",
        "sess1",
        json!({"filter": {"eventMetadata": [
            {"left": {"metadataKey": "stage"}, "operator": "EQUALS_TO"}
        ]}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "EQUALS_TO without a right operand: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "EQUALS_TO without a right operand: expected ValidationException, got {refused:?}"
    );
}

/// `ListSessions` reports exactly the sessions the stored events imply, each with the
/// first event's timestamp as `createdAt`, and pages over them.
#[tokio::test]
async fn list_sessions_reports_the_sessions_events_imply() {
    let router = emulator();
    // Rows: (session, actor, the timestamps of its events). The first timestamp of a
    // session is the one its `createdAt` must carry.
    let sessions = [
        ("sess-a", "actor-1", vec![1_700_000_100.0_f64, 1_700_000_900.0]),
        ("sess-b", "actor-1", vec![1_700_000_200.0]),
        ("sess-c", "actor-1", vec![1_700_000_300.0, 1_700_000_050.0]),
        ("sess-d", "actor-2", vec![1_700_000_400.0]),
    ];
    for (session, actor, stamps) in &sessions {
        for stamp in stamps {
            let (status, created) = create_event(
                &router,
                MEMORY,
                json!({
                    "actorId": actor,
                    "sessionId": session,
                    "eventTimestamp": stamp,
                    "payload": [conversational("USER", "x")],
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "session {session:?}: expected HTTP 201, got {status} with {created:?}"
            );
        }
    }

    for (actor, expected) in [
        (
            "actor-1",
            vec![
                ("sess-c", 1_700_000_300.0_f64),
                ("sess-b", 1_700_000_200.0),
                ("sess-a", 1_700_000_100.0),
            ],
        ),
        ("actor-2", vec![("sess-d", 1_700_000_400.0)]),
        ("actor-unknown", vec![]),
    ] {
        let (status, page) = list_sessions(&router, MEMORY, actor, json!({})).await;
        assert_eq!(
            status, 200,
            "actor {actor:?}: expected HTTP 200, got {status} with {page:?}"
        );
        let seen: Vec<(String, f64)> = page["sessionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                assert_eq!(
                    summary["actorId"].as_str(),
                    Some(actor),
                    "actor {actor:?}: expected each summary to carry the actor, got {summary:?}"
                );
                (
                    summary["sessionId"].as_str().unwrap_or_default().to_string(),
                    summary["createdAt"].as_f64().unwrap_or_default(),
                )
            })
            .collect();
        let want: Vec<(String, f64)> = expected
            .iter()
            .map(|(id, at)| ((*id).to_string(), *at))
            .collect();
        assert_eq!(
            seen, want,
            "actor {actor:?}: expected sessions {want:?} (createdAt = the session's first event), got {seen:?}"
        );
    }

    // Paging over the three sessions of actor-1, for every page size.
    for page_size in [1_usize, 2, 3, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token = Value::Null;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(value) = token.as_str() {
                request["nextToken"] = json!(value);
            }
            let (status, page) = list_sessions(&router, MEMORY, "actor-1", request).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected HTTP 200, got {status} with {page:?}"
            );
            collected.extend(
                page["sessionSummaries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|summary| summary["sessionId"].as_str().unwrap_or_default().to_string()),
            );
            token = page["nextToken"].clone();
            if token.is_null() {
                break;
            }
            assert!(
                collected.len() <= 3,
                "page size {page_size}: expected pagination to terminate, got {collected:?}"
            );
        }
        assert_eq!(
            collected,
            vec!["sess-c", "sess-b", "sess-a"],
            "page size {page_size}: expected every session once, got {collected:?}"
        );
    }

    // `HAS_EVENTS` keeps only the sessions that still hold an event; emptying one
    // through DeleteEvent takes it out of that filtered listing but not out of the
    // unfiltered one.
    let (_, page) = list_events(&router, MEMORY, "actor-1", "sess-b", json!({})).await;
    let only = page["events"][0]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, _) = delete_event(&router, MEMORY, "actor-1", "sess-b", &only).await;
    assert_eq!(status, 200, "expected HTTP 200 from DeleteEvent, got {status}");

    for (case, filter, expected) in [
        ("unfiltered", json!({}), vec!["sess-c", "sess-b", "sess-a"]),
        (
            "has-events",
            json!({"filter": {"eventFilter": "HAS_EVENTS"}}),
            vec!["sess-c", "sess-a"],
        ),
    ] {
        let (status, page) = list_sessions(&router, MEMORY, "actor-1", filter).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {page:?}"
        );
        let seen: Vec<String> = page["sessionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["sessionId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }
}

/// A deleted event is gone from every read, and deleting it again is a 404.
#[tokio::test]
async fn a_deleted_event_disappears_from_get_and_list() {
    let router = emulator();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..3 {
        let (_, created) = create_event(
            &router,
            MEMORY,
            json!({
                "actorId": "actor-1",
                "sessionId": "sess1",
                "eventTimestamp": 1_700_000_000 + index,
                "payload": [conversational("USER", &format!("event {index}"))],
            }),
        )
        .await;
        ids.push(
            created["event"]["eventId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // Delete them one at a time; after each, that id is unreachable and the listing
    // holds exactly the rest.
    for (index, id) in ids.clone().into_iter().enumerate() {
        let (status, deleted) = delete_event(&router, MEMORY, "actor-1", "sess1", &id).await;
        assert_eq!(
            status, 200,
            "event {index}: expected HTTP 200 from DeleteEvent, got {status} with {deleted:?}"
        );
        assert_eq!(
            deleted["eventId"].as_str(),
            Some(id.as_str()),
            "event {index}: expected the deleted eventId back, got {deleted:?}"
        );

        let (status, missing) = get_event(&router, MEMORY, "actor-1", "sess1", &id).await;
        assert_eq!(
            status, 404,
            "event {index}: expected HTTP 404 from GetEvent after the delete, got {status} with {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "event {index}: expected ResourceNotFoundException, got {missing:?}"
        );

        let (status, again) = delete_event(&router, MEMORY, "actor-1", "sess1", &id).await;
        assert_eq!(
            status, 404,
            "event {index}: expected HTTP 404 deleting an already-deleted event, got {status} with {again:?}"
        );

        let (_, page) = list_events(&router, MEMORY, "actor-1", "sess1", json!({})).await;
        let remaining: Vec<String> = page["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| event["eventId"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = ids[index + 1..].iter().rev().cloned().collect();
        assert_eq!(
            remaining, expected,
            "event {index}: expected the listing to hold {expected:?}, got {remaining:?}"
        );
    }
}

/// A path that names something that was never stored is a `ResourceNotFoundException`
/// (404) on every read and on the delete — not a 501 and not a 500.
#[tokio::test]
async fn an_unknown_path_is_a_resource_not_found() {
    let router = emulator();
    let (_, created) = create_event(
        &router,
        MEMORY,
        json!({
            "actorId": "actor-1",
            "sessionId": "sess1",
            "eventTimestamp": 1_700_000_000,
            "payload": [conversational("USER", "hello")],
        }),
    )
    .await;
    let known = created["event"]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // Another memory id matching `MemoryId`'s pattern that no event was stored under.
    let other_memory = "other-memory-9876543210";

    // Rows: each step of the path, wrong in turn. `ListEvents` names no event, so its
    // listing is only expected to 404 on the rows that break the session path.
    let cases: Vec<(&str, &str, &str, &str, &str, bool)> = vec![
        (
            "unknown-memory",
            other_memory,
            "actor-1",
            "sess1",
            &known,
            true,
        ),
        ("unknown-actor", MEMORY, "actor-404", "sess1", &known, true),
        ("unknown-session", MEMORY, "actor-1", "sess404", &known, true),
        ("unknown-event", MEMORY, "actor-1", "sess1", "1#abcdef", false),
    ];
    for (case, memory, actor, session, event, session_path_broken) in cases {
        let mut answers = vec![
            {
                let (status, body) = get_event(&router, memory, actor, session, event).await;
                ("GetEvent", status, body)
            },
            {
                let (status, body) = delete_event(&router, memory, actor, session, event).await;
                ("DeleteEvent", status, body)
            },
        ];
        if session_path_broken {
            let (status, body) = list_events(&router, memory, actor, session, json!({})).await;
            answers.push(("ListEvents", status, body));
        }
        for (operation, status, body) in answers {
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected HTTP 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {body:?}"
            );
        }
    }

    // ListSessions addresses the memory, so an unknown memory is a 404 while an actor
    // with no sessions in a known memory is an empty list.
    let (status, body) = list_sessions(&router, other_memory, "actor-1", json!({})).await;
    assert_eq!(
        status, 404,
        "ListSessions on an unknown memory: expected HTTP 404, got {status} with {body:?}"
    );
    let (status, body) = list_sessions(&router, MEMORY, "actor-404", json!({})).await;
    assert_eq!(
        status, 200,
        "ListSessions for an actor with no sessions: expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["sessionSummaries"],
        json!([]),
        "ListSessions for an actor with no sessions: expected an empty list, got {body:?}"
    );
}

/// One memory container, whether addressed by its bare id or by its ARN: `MemoryId`'s
/// pattern admits both spellings.
#[tokio::test]
async fn a_memory_arn_addresses_the_same_container_as_its_id() {
    let router = emulator();
    let (status, created) = create_event(
        &router,
        MEMORY_ARN,
        json!({
            "actorId": "actor-1",
            "sessionId": "sess1",
            "eventTimestamp": 1_700_000_000,
            "payload": [conversational("USER", "hello")],
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected HTTP 201 creating through the ARN, got {status} with {created:?}"
    );
    let event_id = created["event"]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, memory) in [("bare-id", MEMORY), ("arn", MEMORY_ARN)] {
        let (status, fetched) = get_event(&router, memory, "actor-1", "sess1", &event_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the event to be reachable, got {status} with {fetched:?}"
        );
        assert_eq!(
            fetched["event"]["eventId"].as_str(),
            Some(event_id.as_str()),
            "case {case:?}: expected the same event, got {fetched:?}"
        );
    }
}

/// An input outside a modelled constraint, and a `nextToken` this service never
/// issued, are refused with a 400 rather than accepted or answered with a 5xx.
#[tokio::test]
async fn out_of_constraint_input_is_refused_with_400() {
    let router = emulator();
    let (_, created) = create_event(
        &router,
        MEMORY,
        json!({
            "actorId": "actor-1",
            "sessionId": "sess1",
            "eventTimestamp": 1_700_000_000,
            "payload": [conversational("USER", "hello")],
        }),
    )
    .await;
    assert!(
        created["event"]["eventId"].is_string(),
        "the fixture event must exist, got {created:?}"
    );

    // `PayloadTypeList` is length ..100 and `MetadataMap` is length 0..15 (source:
    // smithy_query shape="PayloadTypeList" / "MetadataMap"); `MaxResults` is range
    // 1..100. Each row sits one step past a published bound, and the row before it is
    // the bound itself.
    let payload_of = |count: usize| -> Value {
        Value::Array((0..count).map(|i| conversational("USER", &format!("item {i}"))).collect())
    };
    let metadata_of = |count: usize| -> Value {
        Value::Object(
            (0..count)
                .map(|i| (format!("key{i}"), json!({"stringValue": "v"})))
                .collect(),
        )
    };

    for (case, request, expected_status) in [
        ("payload-at-the-bound", json!({"payload": payload_of(100)}), 201),
        ("payload-past-the-bound", json!({"payload": payload_of(101)}), 400),
        (
            "metadata-at-the-bound",
            json!({"payload": payload_of(1), "metadata": metadata_of(15)}),
            201,
        ),
        (
            "metadata-past-the-bound",
            json!({"payload": payload_of(1), "metadata": metadata_of(16)}),
            400,
        ),
    ] {
        let mut body = json!({
            "actorId": "actor-1",
            "sessionId": "sess-bounds",
            "eventTimestamp": 1_700_000_000,
        });
        for (key, value) in request.as_object().expect("an object row") {
            body[key] = value.clone();
        }
        let (status, answer) = create_event(&router, MEMORY, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {answer:?}"
        );
    }

    // `MaxResults` range 1..100: both published bounds are accepted, one step past
    // either is refused.
    for (case, max_results, expected_status) in [
        ("max-results-min", 1, 200),
        ("max-results-max", 100, 200),
        ("max-results-below-min", 0, 400),
        ("max-results-above-max", 101, 400),
    ] {
        for (operation, (status, body)) in [
            (
                "ListEvents",
                list_events(
                    &router,
                    MEMORY,
                    "actor-1",
                    "sess1",
                    json!({"maxResults": max_results}),
                )
                .await,
            ),
            (
                "ListSessions",
                list_sessions(&router, MEMORY, "actor-1", json!({"maxResults": max_results})).await,
            ),
        ] {
            assert_eq!(
                status, expected_status,
                "case {case:?} on {operation}: expected HTTP {expected_status}, got {status} with {body:?}"
            );
        }
    }

    // `EventId`'s pattern is `^[0-9]+#[a-fA-F0-9]+$`: a path id that does not match is
    // a 400, not a 404.
    for (case, event_id) in [
        ("no-separator", "deadbeef"),
        ("non-hex-suffix", "123#zzz"),
        ("empty-prefix", "#abc"),
    ] {
        let (status, body) = get_event(&router, MEMORY, "actor-1", "sess1", event_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an eventId outside its pattern, got {status} with {body:?}"
        );
    }

    // A `nextToken` this service never issued cannot be resumed.
    let other_lists_token = aws_smithy_types::base64::encode("sessions:1");
    for (case, token) in [
        ("not-base64", "not a token"),
        ("base64-but-not-a-cursor", "YWJjZGU="),
        ("the-other-list-s-token", other_lists_token.as_str()),
    ] {
        let (status, body) = list_events(
            &router,
            MEMORY,
            "actor-1",
            "sess1",
            json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an unknown nextToken, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // `EventMetadataFilterList` is modelled @length(1..5): five expressions are
    // accepted, six are refused, so the work one request asks for stays bounded.
    let expression = json!({"left": {"metadataKey": "stage"}, "operator": "EXISTS"});
    for (case, count, expected_status) in [("at-the-bound", 5, 200), ("past-the-bound", 6, 400)] {
        let expressions = Value::Array(vec![expression.clone(); count]);
        let (status, body) = list_events(
            &router,
            MEMORY,
            "actor-1",
            "sess1",
            json!({"filter": {"eventMetadata": expressions}}),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {count} eventMetadata expressions, got {status} with {body:?}"
        );
    }
}

/// The store has a ceiling: once a bound is passed the least recently written record
/// is evicted, so request volume alone cannot grow it without limit.
#[tokio::test]
async fn the_store_evicts_its_least_recently_written_records() {
    use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::state::{
        MAX_EVENTS_PER_SESSION, MAX_MEMORIES,
    };
    let router = emulator();

    // One event in each of MAX_MEMORIES + 1 distinct memory containers, oldest first.
    // Each id matches `MemoryId`'s pattern `[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}`.
    let mut written: Vec<(String, String)> = Vec::new();
    for index in 0..=MAX_MEMORIES {
        let memory = format!("mem-{index:03}-0123456789");
        let (status, created) = create_event(
            &router,
            &memory,
            json!({
                "actorId": "actor-1",
                "sessionId": "sess1",
                "eventTimestamp": 1_700_000_000,
                "payload": [conversational("USER", "hello")],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "memory {index}: expected HTTP 201, got {status} with {created:?}"
        );
        written.push((
            memory,
            created["event"]["eventId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    }

    // The first container written is the one evicted; every later one is still served.
    for (index, (memory, event_id)) in written.iter().enumerate() {
        let expected = if index == 0 { 404 } else { 200 };
        let (status, body) = get_event(&router, memory, "actor-1", "sess1", event_id).await;
        assert_eq!(
            status, expected,
            "memory {index} ({memory}): expected HTTP {expected} after {} memories were written, got {status} with {body:?}",
            written.len()
        );
    }

    // And inside one session: the oldest event leaves once the session passes its bound.
    let memory = "ring-memory-0123456789";
    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_EVENTS_PER_SESSION + 3 {
        let (status, created) = create_event(
            &router,
            memory,
            json!({
                "actorId": "actor-1",
                "sessionId": "sess1",
                "eventTimestamp": 1_700_000_000,
                "payload": [conversational("USER", "x")],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "event {index}: expected HTTP 201, got {status} with {created:?}"
        );
        ids.push(
            created["event"]["eventId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    for (case, event_id, expected) in [
        ("oldest", &ids[0], 404),
        ("third-oldest", &ids[2], 404),
        ("first-retained", &ids[3], 200),
        ("newest", ids.last().expect("at least one event"), 200),
    ] {
        let (status, body) = get_event(&router, memory, "actor-1", "sess1", event_id).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} after {} events in one session, got {status} with {body:?}",
            ids.len()
        );
    }
}
