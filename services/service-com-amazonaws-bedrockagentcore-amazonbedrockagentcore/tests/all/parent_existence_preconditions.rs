// SPDX-License-Identifier: CC0-1.0

//! The EXISTENCE PRE-CONDITION of every write this data plane serves.
//!
//! THE RULE: a data-plane write whose request path names a resource the AgentCore CONTROL
//! plane owns resolves that identifier against what the control plane created BEFORE it
//! persists anything or mints an id, and refuses an identifier nothing created with
//! `ResourceNotFoundException` (`@httpError 404`) whose `message` is that resource's label,
//! `": "`, then the identifier exactly as the request spelled it. Only the code interpreter
//! the service provides to every account resolves without a create.
//!
//! The rows span what the rule generalises over:
//!
//! * which OPERATION writes — `CreateEvent`, `IngestData` and `BatchCreateMemoryRecords` in
//!   the Memory plane, `StartCodeInterpreterSession` and `InvokeCodeInterpreter` in the
//!   tools plane — since the pre-condition is the service's and not one handler's;
//! * which RESOURCE the path names: a memory container or a code interpreter;
//! * which FORM the identifier arrived in: the bare id, the `arn:…:memory/<id>` spelling
//!   `MemoryId` also admits, and a dotted identifier shaped like the service's own tool;
//! * whether the parent EXISTS when the call arrives: never created, created, or created
//!   and then deleted.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the routes, status codes and `restJson1` error bodies are exercised and not
//! just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

use super::harness::{create_code_interpreter, create_memory, delete_code_interpreter, label};

/// The error code every refusal under test carries.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// A memory id matching `MemoryId`'s pattern
/// `^(arn:…:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, which this suite
/// creates through the control plane's inventory.
const MEMORY: &str = "live-memory-0123456789";
/// The same container addressed by the ARN spelling that pattern also admits.
const MEMORY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/live-memory-0123456789";
/// A second id of the same shape that nothing ever creates, and its ARN spelling.
const GHOST_MEMORY: &str = "ghost-memory-9876543210";
const GHOST_MEMORY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/ghost-memory-9876543210";

/// A code interpreter id matching `CodeInterpreterId`'s customer-created alternative
/// `[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}`, which this suite creates, and a second of
/// the same shape that nothing creates.
const INTERPRETER: &str = "live_interpreter-1a2b3c4d5e";
const GHOST_INTERPRETER: &str = "ghost_interpreter-9z8y7x6w5v";
/// An identifier shaped like the tool the service provides but naming no version it does:
/// the reserved spelling is `aws.codeinterpreter.v1` only.
const GHOST_SYSTEM_INTERPRETER: &str = "aws.codeinterpreter.v2";
/// The code interpreter the service provides to every account, which no create mints.
const SYSTEM_INTERPRETER: &str = "aws.codeinterpreter.v1";

/// The actor and session every write in this suite addresses.
const ACTOR: &str = "actor-1";
const SESSION: &str = "sess1";

/// The resource label a refusal is expected to name. Spelled out here rather than read
/// off the emulator, or the assertion would only re-state whatever the implementation
/// happens to carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    Memory,
    CodeInterpreter,
}

impl Resource {
    fn label(self) -> &'static str {
        match self {
            Self::Memory => "Memory",
            Self::CodeInterpreter => "Code Interpreter",
        }
    }
}

/// One write of this data plane, addressed by the identifier of the parent it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    /// `POST /memories/{memoryId}/events`
    CreateEvent,
    /// `POST /memories/{memoryId}/ingest`
    IngestData,
    /// `POST /memories/{memoryId}/memoryRecords/batchCreate`
    BatchCreateMemoryRecords,
    /// `PUT /code-interpreters/{codeInterpreterIdentifier}/sessions/start`
    StartCodeInterpreterSession,
    /// `POST /code-interpreters/{codeInterpreterIdentifier}/tools/invoke`
    InvokeCodeInterpreter,
}

/// Every write under test, so no row set can quietly leave one out.
const EVERY_WRITE: [Write; 5] = [
    Write::CreateEvent,
    Write::IngestData,
    Write::BatchCreateMemoryRecords,
    Write::StartCodeInterpreterSession,
    Write::InvokeCodeInterpreter,
];

impl Write {
    fn operation(self) -> &'static str {
        match self {
            Self::CreateEvent => "CreateEvent",
            Self::IngestData => "IngestData",
            Self::BatchCreateMemoryRecords => "BatchCreateMemoryRecords",
            Self::StartCodeInterpreterSession => "StartCodeInterpreterSession",
            Self::InvokeCodeInterpreter => "InvokeCodeInterpreter",
        }
    }

    /// The resource this write's path names, which is the one a refusal reports.
    fn parent(self) -> Resource {
        match self {
            Self::CreateEvent | Self::IngestData | Self::BatchCreateMemoryRecords => {
                Resource::Memory
            }
            Self::StartCodeInterpreterSession | Self::InvokeCodeInterpreter => {
                Resource::CodeInterpreter
            }
        }
    }

    /// The status this write answers when its parent exists (`@http` code for the three
    /// that declare one, 200 for the two that do not).
    fn success_status(self) -> u16 {
        match self {
            Self::CreateEvent | Self::BatchCreateMemoryRecords => 201,
            Self::IngestData => 202,
            Self::StartCodeInterpreterSession | Self::InvokeCodeInterpreter => 200,
        }
    }

    /// An identifier of the right shape that this suite never creates, for the row set
    /// that calls every write on a parent nothing created.
    fn ghost_parent(self) -> &'static str {
        match self.parent() {
            Resource::Memory => GHOST_MEMORY,
            Resource::CodeInterpreter => GHOST_INTERPRETER,
        }
    }

    /// The identifier of the parent this suite creates, for the row set that calls every
    /// write once that parent exists.
    fn live_parent(self) -> &'static str {
        match self.parent() {
            Resource::Memory => MEMORY,
            Resource::CodeInterpreter => INTERPRETER,
        }
    }

    async fn send(self, router: &MultiServiceRouter, parent: &str) -> (u16, Value) {
        match self {
            Self::CreateEvent => {
                send(
                    router,
                    "POST",
                    &format!("/memories/{}/events", label(parent)),
                    Some(json!({
                        "actorId": ACTOR,
                        "sessionId": SESSION,
                        "eventTimestamp": 1_700_000_000,
                        "payload": [{"conversational": {
                            "role": "USER",
                            "content": {"text": "hello"},
                        }}],
                    })),
                )
                .await
            }
            Self::IngestData => {
                send(
                    router,
                    "POST",
                    &format!("/memories/{}/ingest", label(parent)),
                    Some(json!({
                        "actorId": ACTOR,
                        "sessionId": SESSION,
                        "contentTimestamp": 1_700_000_000,
                        "source": {"inline": {"payload": [{"conversational": {
                            "role": "USER",
                            "content": {"text": "remember this"},
                        }}]}},
                    })),
                )
                .await
            }
            Self::BatchCreateMemoryRecords => {
                send(
                    router,
                    "POST",
                    &format!("/memories/{}/memoryRecords/batchCreate", label(parent)),
                    Some(json!({"records": [{
                        "requestIdentifier": "req-1",
                        "namespaces": ["support/facts/actor-1"],
                        "content": {"text": "a fact"},
                        "timestamp": 1_700_000_000,
                    }]})),
                )
                .await
            }
            Self::StartCodeInterpreterSession => {
                send(
                    router,
                    "PUT",
                    &format!("/code-interpreters/{}/sessions/start", label(parent)),
                    Some(json!({"name": "probe", "sessionTimeoutSeconds": 900})),
                )
                .await
            }
            // No `sessionId`, which is the call that would otherwise START a session on
            // the interpreter it names.
            Self::InvokeCodeInterpreter => {
                send(
                    router,
                    "POST",
                    &format!("/code-interpreters/{}/tools/invoke", label(parent)),
                    Some(json!({"name": "listFiles", "arguments": {"path": ""}})),
                )
                .await
            }
        }
    }
}

/// The emulator and the shared engine beside it, with request authentication off so a test
/// signs nothing. Nothing is created here: each case creates the parents it needs.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an empty
/// body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
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

/// How many memory containers and code interpreters this plane's own store holds, which is
/// how a case shows that a refused write persisted nothing at all.
fn stored_parents(engine: &engine::InMemoryEngine) -> (usize, usize) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    (store.memories.len(), store.code_interpreters.len())
}

/// Every write naming a parent nothing created is refused with 404
/// `ResourceNotFoundException`, worded with that resource's label and the identifier the
/// request spelled — and persists nothing.
#[tokio::test]
async fn a_write_naming_a_parent_nothing_created_is_refused_with_404() {
    let (router, engine) = emulator();

    // Rows: every write on a parent of its own kind that nothing created, then the other
    // identifier forms the rule has to hold for.
    let mut cases: Vec<(String, Write, String)> = EVERY_WRITE
        .iter()
        .map(|write| {
            (
                format!("{}-on-a-parent-never-created", write.operation()),
                *write,
                write.ghost_parent().to_string(),
            )
        })
        .collect();
    cases.extend([
        (
            "CreateEvent-on-the-arn-spelling-of-a-container-never-created".to_string(),
            Write::CreateEvent,
            GHOST_MEMORY_ARN.to_string(),
        ),
        (
            "IngestData-on-the-arn-spelling-of-a-container-never-created".to_string(),
            Write::IngestData,
            GHOST_MEMORY_ARN.to_string(),
        ),
        (
            "BatchCreateMemoryRecords-on-the-arn-spelling-of-a-container-never-created"
                .to_string(),
            Write::BatchCreateMemoryRecords,
            GHOST_MEMORY_ARN.to_string(),
        ),
        (
            "StartCodeInterpreterSession-on-a-version-of-the-service-tool-that-does-not-exist"
                .to_string(),
            Write::StartCodeInterpreterSession,
            GHOST_SYSTEM_INTERPRETER.to_string(),
        ),
        (
            "InvokeCodeInterpreter-on-a-version-of-the-service-tool-that-does-not-exist"
                .to_string(),
            Write::InvokeCodeInterpreter,
            GHOST_SYSTEM_INTERPRETER.to_string(),
        ),
    ]);

    for (case, write, parent) in cases {
        let (status, body) = write.send(&router, &parent).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            RESOURCE_NOT_FOUND,
            "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {body:?}"
        );
        let expected = format!("{} not found: {parent}", write.parent().label());
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message(&body)
        );
    }

    // Not one of those refusals left anything behind: no container and no code interpreter
    // was conjured by a write that names one.
    let (memories, interpreters) = stored_parents(&engine);
    assert_eq!(
        (memories, interpreters),
        (0, 0),
        "case a refused write: expected nothing persisted, got {memories} container(s) and \
         {interpreters} code interpreter(s)"
    );
}

/// The same writes succeed once the control plane has created the parent, and what each one
/// wrote reads back: the pre-condition refuses an absent parent and nothing more.
#[tokio::test]
async fn the_same_write_succeeds_once_the_parent_exists_and_round_trips() {
    let (router, engine) = emulator();
    create_memory(&engine, MEMORY);
    create_code_interpreter(&engine, INTERPRETER);

    let mut answered: Vec<(Write, Value)> = Vec::new();
    for write in EVERY_WRITE {
        let parent = write.live_parent();
        let (status, body) = write.send(&router, parent).await;
        assert_eq!(
            status,
            write.success_status(),
            "case {}-on-a-created-parent: expected HTTP {}, got {status} with {body:?}",
            write.operation(),
            write.success_status()
        );
        answered.push((write, body));
    }

    // The created event reads back through `GetEvent`.
    let event = answered
        .iter()
        .find(|(write, _)| *write == Write::CreateEvent)
        .map(|(_, body)| body["event"]["eventId"].as_str().unwrap_or_default().to_string())
        .expect("CreateEvent answered");
    let (status, read) = send(
        &router,
        "GET",
        &format!(
            "/memories/{}/actor/{}/sessions/{}/events/{}",
            label(MEMORY),
            label(ACTOR),
            label(SESSION),
            label(&event)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the created event: expected GetEvent to report it, got {status} with {read:?}"
    );
    assert_eq!(
        read["event"]["eventId"].as_str(),
        Some(event.as_str()),
        "case the created event: expected the stored event back, got {read:?}"
    );

    // The created memory record reads back through `GetMemoryRecord`.
    let record = answered
        .iter()
        .find(|(write, _)| *write == Write::BatchCreateMemoryRecords)
        .map(|(_, body)| {
            body["successfulRecords"][0]["memoryRecordId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .expect("BatchCreateMemoryRecords answered");
    let (status, read) = send(
        &router,
        "GET",
        &format!(
            "/memories/{}/memoryRecord/{}",
            label(MEMORY),
            label(&record)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the created memory record: expected GetMemoryRecord to report it, got {status} \
         with {read:?}"
    );

    // The started session reads back through `GetCodeInterpreterSession`.
    let session = answered
        .iter()
        .find(|(write, _)| *write == Write::StartCodeInterpreterSession)
        .map(|(_, body)| body["sessionId"].as_str().unwrap_or_default().to_string())
        .expect("StartCodeInterpreterSession answered");
    let (status, read) = send(
        &router,
        "GET",
        &format!(
            "/code-interpreters/{}/sessions/get?sessionId={}",
            label(INTERPRETER),
            label(&session)
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the started session: expected GetCodeInterpreterSession to report it, got \
         {status} with {read:?}"
    );
    assert_eq!(
        read["sessionId"].as_str(),
        Some(session.as_str()),
        "case the started session: expected the stored session back, got {read:?}"
    );

    // The ingested content's session is one the memory now reports.
    let (status, sessions) = send(
        &router,
        "POST",
        &format!(
            "/memories/{}/actor/{}/sessions",
            label(MEMORY),
            label(ACTOR)
        ),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the ingested content: expected ListSessions to answer, got {status} with \
         {sessions:?}"
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
        "case the ingested content: expected the session {SESSION:?} among {listed:?}"
    );

    // Either spelling of the created container is the same container, so the ARN form of
    // the same id is accepted by every memory-plane write too.
    for write in [
        Write::CreateEvent,
        Write::IngestData,
        Write::BatchCreateMemoryRecords,
    ] {
        let (status, body) = write.send(&router, MEMORY_ARN).await;
        assert_eq!(
            status,
            write.success_status(),
            "case {}-addressed-by-the-container-arn: expected HTTP {}, got {status} with \
             {body:?}",
            write.operation(),
            write.success_status()
        );
    }

    // And the code interpreter the service provides needs no create at all.
    for write in [
        Write::StartCodeInterpreterSession,
        Write::InvokeCodeInterpreter,
    ] {
        let (status, body) = write.send(&router, SYSTEM_INTERPRETER).await;
        assert_eq!(
            status,
            write.success_status(),
            "case {}-on-the-service-provided-tool: expected HTTP {}, got {status} with \
             {body:?}",
            write.operation(),
            write.success_status()
        );
    }
}

/// A code interpreter the control plane has DELETED stops resolving: the write that
/// succeeded while it existed is refused again, with the same wording as one that never
/// existed.
#[tokio::test]
async fn a_write_on_a_deleted_code_interpreter_is_refused_again() {
    let (router, engine) = emulator();
    create_code_interpreter(&engine, INTERPRETER);

    let (status, started) = Write::StartCodeInterpreterSession
        .send(&router, INTERPRETER)
        .await;
    assert_eq!(
        status, 200,
        "case before the delete: expected the session to start, got {status} with {started:?}"
    );

    delete_code_interpreter(&engine, INTERPRETER);

    for write in [
        Write::StartCodeInterpreterSession,
        Write::InvokeCodeInterpreter,
    ] {
        let (status, body) = write.send(&router, INTERPRETER).await;
        assert_eq!(
            status, 404,
            "case {}-after-the-delete: expected HTTP 404, got {status} with {body:?}",
            write.operation()
        );
        let expected = format!("Code Interpreter not found: {INTERPRETER}");
        assert_eq!(
            message(&body),
            expected,
            "case {}-after-the-delete: expected message {expected:?}, got {:?}",
            write.operation(),
            message(&body)
        );
    }
}
