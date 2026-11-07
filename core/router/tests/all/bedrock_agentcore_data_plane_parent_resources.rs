// SPDX-License-Identifier: CC0-1.0

//! The AgentCore DATA plane writes only inside resources the CONTROL plane created.
//!
//! RULE: a data-plane write whose request path names a resource the AgentCore control plane
//! owns — a Memory container, a code interpreter — resolves that identifier against what
//! that plane created BEFORE it persists anything, and refuses an identifier nothing
//! created with `ResourceNotFoundException` (HTTP 404). The two planes are separate
//! services over one shared engine, and the data plane's own API creates neither resource:
//! a `CreateEvent` names a `memoryId` and nothing else. So the identifier the data plane
//! accepts is exactly the one the control plane MINTED, which is why this suite lives here
//! rather than in either service's own crate — a single-service router has no control plane
//! to create anything.
//!
//! The rows span what the rule generalises over: which resource family the write names
//! (memory container, code interpreter), which operation writes (`CreateEvent`,
//! `IngestData`, `BatchCreateMemoryRecords`, `StartCodeInterpreterSession`), whether the id
//! came from a create or was invented, and whether the resource still exists (a deleted
//! code interpreter stops resolving).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// An id of the modelled shape that no create ever mints here.
const GHOST_MEMORY: &str = "ghost-memory-9876543210";
/// An id of the code interpreter shape that no create ever mints here.
const GHOST_INTERPRETER: &str = "ghost_interpreter-9z8y7x6w5v";

/// The actor and session the writes address.
const ACTOR: &str = "actor-1";
const SESSION: &str = "sess1";

/// AgentCore Control and the AgentCore data plane over ONE shared engine, exactly as
/// `router/src/lib.rs` mounts them: the two share a signing name and are told apart by the
/// route each model binds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService::new(engine),
    );
    router
}

/// One restJson1 call, signed for the shared signing name; the ROUTE is what picks the
/// plane that answers it.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("bedrock-agentcore.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Percent-encode one `@httpLabel` value the way an AWS SDK does.
fn label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `CreateMemory` on the control plane: `POST /memories/create`, answering the `memoryId`
/// it minted.
async fn create_memory(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/memories/create",
        Some(json!({"name": name, "eventExpiryDuration": 30})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateMemory must answer 202, got {status} with {created:?}"
    );
    created["memory"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateMemory reported no memory id: {created:?}"))
        .to_string()
}

/// `CreateCodeInterpreter` on the control plane: `PUT /code-interpreters`, answering the
/// `codeInterpreterId` it minted.
async fn create_code_interpreter(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "PUT",
        "/code-interpreters",
        Some(json!({"name": name, "networkConfiguration": {"networkMode": "SANDBOX"}})),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateCodeInterpreter must answer 202, got {status} with {created:?}"
    );
    created["codeInterpreterId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCodeInterpreter reported no id: {created:?}"))
        .to_string()
}

/// `CreateEvent` on the data plane: `POST /memories/{memoryId}/events`.
async fn create_event(router: &MultiServiceRouter, memory: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/memories/{}/events", label(memory)),
        Some(json!({
            "actorId": ACTOR,
            "sessionId": SESSION,
            "eventTimestamp": 1_700_000_000,
            "payload": [{"conversational": {"role": "USER", "content": {"text": "hello"}}}],
        })),
    )
    .await
}

/// `IngestData` on the data plane: `POST /memories/{memoryId}/ingest`.
async fn ingest_data(router: &MultiServiceRouter, memory: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/memories/{}/ingest", label(memory)),
        Some(json!({
            "actorId": ACTOR,
            "sessionId": SESSION,
            "contentTimestamp": 1_700_000_000,
            "source": {"inline": {"payload": [
                {"conversational": {"role": "USER", "content": {"text": "remember this"}}}
            ]}},
        })),
    )
    .await
}

/// `BatchCreateMemoryRecords` on the data plane:
/// `POST /memories/{memoryId}/memoryRecords/batchCreate`.
async fn batch_create_records(router: &MultiServiceRouter, memory: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/memories/{}/memoryRecords/batchCreate", label(memory)),
        Some(json!({"records": [{
            "requestIdentifier": "req-1",
            "namespaces": ["support/facts/actor-1"],
            "content": {"text": "a fact"},
            "timestamp": 1_700_000_000,
        }]})),
    )
    .await
}

/// `StartCodeInterpreterSession` on the data plane:
/// `PUT /code-interpreters/{codeInterpreterIdentifier}/sessions/start`.
async fn start_session(router: &MultiServiceRouter, interpreter: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/code-interpreters/{}/sessions/start", label(interpreter)),
        Some(json!({"name": "probe", "sessionTimeoutSeconds": 900})),
    )
    .await
}

/// One of the data plane's memory-plane writes, so the rows below can name them side by
/// side.
#[derive(Debug, Clone, Copy)]
enum MemoryWrite {
    CreateEvent,
    IngestData,
    BatchCreateMemoryRecords,
}

impl MemoryWrite {
    fn operation(self) -> &'static str {
        match self {
            Self::CreateEvent => "CreateEvent",
            Self::IngestData => "IngestData",
            Self::BatchCreateMemoryRecords => "BatchCreateMemoryRecords",
        }
    }

    /// The status this write answers for a container that exists (each one's `@http` code).
    fn success_status(self) -> u16 {
        match self {
            Self::CreateEvent | Self::BatchCreateMemoryRecords => 201,
            Self::IngestData => 202,
        }
    }

    async fn send(self, router: &MultiServiceRouter, memory: &str) -> (u16, Value) {
        match self {
            Self::CreateEvent => create_event(router, memory).await,
            Self::IngestData => ingest_data(router, memory).await,
            Self::BatchCreateMemoryRecords => batch_create_records(router, memory).await,
        }
    }
}

/// A memory container the control plane created takes the data plane's writes, and the
/// events written into it read back; an id nothing created takes none of them.
#[tokio::test]
async fn the_memory_plane_writes_only_into_a_container_the_control_plane_created() {
    let router = emulator();
    let memory = create_memory(&router, "crossplane_memory").await;

    for write in [
        MemoryWrite::CreateEvent,
        MemoryWrite::IngestData,
        MemoryWrite::BatchCreateMemoryRecords,
    ] {
        let case = write.operation();
        let expected = write.success_status();
        let (status, body) = write.send(&router, &memory).await;
        assert_eq!(
            status, expected,
            "case {case:?} in the created container {memory}: expected HTTP {expected}, got \
             {status} with {body:?}"
        );

        // The same call, on an id no create minted, is the modelled 404 — worded with the
        // identifier the request spelled.
        let (status, refused) = write.send(&router, GHOST_MEMORY).await;
        assert_eq!(
            status, 404,
            "case {case:?} on a container never created: expected HTTP 404, got {status} with \
             {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?} on a container never created: expected ResourceNotFoundException, \
             got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("Memory not found: {GHOST_MEMORY}"),
            "case {case:?} on a container never created: expected the documented message, got \
             {:?}",
            message(&refused)
        );
    }

    // What the first write stored is readable through the data plane's own read, so the
    // accepted write really did land in the created container.
    let (status, sessions) = call(
        &router,
        "POST",
        &format!(
            "/memories/{}/actor/{}/sessions",
            label(&memory),
            label(ACTOR)
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListSessions on the created container must answer 200, got {status} with {sessions:?}"
    );
    let listed: Vec<String> = sessions["sessionSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["sessionId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        listed.contains(&SESSION.to_string()),
        "the written session must be reported, expected {SESSION:?} among {listed:?}"
    );
}

/// A code interpreter the control plane created takes a session; one nothing created takes
/// none, and neither does one the control plane has deleted.
#[tokio::test]
async fn the_tools_plane_starts_a_session_only_on_a_code_interpreter_that_exists() {
    let router = emulator();
    let interpreter = create_code_interpreter(&router, "crossplane_interpreter").await;

    let (status, started) = start_session(&router, &interpreter).await;
    assert_eq!(
        status, 200,
        "a session on the created interpreter {interpreter} must start, got {status} with \
         {started:?}"
    );
    let session = started["sessionId"].as_str().unwrap_or_default().to_string();
    let (status, read) = call(
        &router,
        "GET",
        &format!(
            "/code-interpreters/{}/sessions/get?sessionId={}",
            label(&interpreter),
            label(&session)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the started session must read back, got {status} with {read:?}"
    );

    // An interpreter no create minted takes no session.
    let (status, refused) = start_session(&router, GHOST_INTERPRETER).await;
    assert_eq!(
        status, 404,
        "case an interpreter never created: expected HTTP 404, got {status} with {refused:?}"
    );
    assert_eq!(
        message(&refused),
        format!("Code Interpreter not found: {GHOST_INTERPRETER}"),
        "case an interpreter never created: expected the documented message, got {:?}",
        message(&refused)
    );

    // And one the control plane deletes stops taking them.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/code-interpreters/{}", label(&interpreter)),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "DeleteCodeInterpreter must answer 202, got {status} with {deleted:?}"
    );
    let (status, refused) = start_session(&router, &interpreter).await;
    assert_eq!(
        status, 404,
        "case a deleted interpreter: expected HTTP 404, got {status} with {refused:?}"
    );
    assert_eq!(
        message(&refused),
        format!("Code Interpreter not found: {interpreter}"),
        "case a deleted interpreter: expected the documented message, got {:?}",
        message(&refused)
    );
}
