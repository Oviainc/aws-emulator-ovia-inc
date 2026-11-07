// SPDX-License-Identifier: CC0-1.0

//! How this service REFUSES a lookup that finds nothing.
//!
//! THE RULE: a `ResourceNotFoundException` (`@httpError 404`, one `message` member —
//! `smithy_query(query="shape",shape="ResourceNotFoundException")`) words its message as
//! the capitalised label of the resource that did not resolve, then `": "`, then THAT
//! resource's identifier exactly as the request spelled it — and nothing else, so no
//! identifier of an enclosing resource that DID resolve appears in it. A path-addressed
//! resource is resolved outside-in, so the label and identifier belong to the OUTERMOST
//! step of the path that does not exist.
//!
//! The rows span what the rule generalises over:
//!
//! * which OPERATION refuses (`ListSessions`, `ListEvents`, `GetEvent`, `DeleteEvent` in
//!   the Memory event plane, `GetCodeInterpreterSession` in the tools plane, `Evaluate` in
//!   the evaluation plane), since the wording is the service's and not one handler's;
//! * which RESOURCE of the path is missing (the memory container, the session, the event),
//!   including a path whose inner identifiers are also unknown — the memory is still what
//!   is reported;
//! * which FORM the identifier arrived in: `MemoryId`'s pattern admits both the bare id
//!   and the `arn:…:memory/<id>` spelling, and whichever one was sent is the one echoed.
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
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// A memory id matching `MemoryId`'s pattern, which this suite stores an event under.
const MEMORY: &str = "demo-memory-0123456789";
/// A second id of the same shape that nothing is ever stored under.
const GHOST_MEMORY: &str = "ghost-memory-9876543210";
/// The same absent container addressed by the ARN spelling `MemoryId` also admits.
const GHOST_MEMORY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/ghost-memory-9876543210";

/// The actor and session the stored event belongs to.
const ACTOR: &str = "actor-1";
const SESSION: &str = "sess1";

/// An actor id and a session id of the modelled shape that hold nothing.
const GHOST_ACTOR: &str = "actor-404";
const GHOST_SESSION: &str = "sess404";

/// An `EventId` matching the modelled pattern `^[0-9]+#[a-fA-F0-9]+$` that no stored
/// event carries.
const GHOST_EVENT: &str = "1#abcdef";

/// The system code interpreter AWS ships, which this suite starts a session on.
const INTERPRETER: &str = "aws.codeinterpreter.v1";
/// A second code interpreter this emulator holds no session for at all.
const OTHER_INTERPRETER: &str = "demo_interpreter-1a2b3c4d5e";

/// A `CodeInterpreterSessionId` (`^[0-9a-zA-Z]{1,40}$`) no session was started under.
const GHOST_CODE_SESSION: &str = "ghostsession0000";

/// A built-in evaluator the service itself provides, and a custom evaluator id no
/// control plane created here (source: smithy_query query="shape" shape="EvaluatorArn",
/// whose account-free alternative covers the `Builtin.`/`ThirdParty.` families only).
const EVALUATOR: &str = "Builtin.Correctness";
const GHOST_EVALUATOR: &str = "quality-checker-1a2b3c4d5e";

/// A trace id and a span id of the modelled shapes, for the one span `Evaluate` is sent.
const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const SPAN: &str = "00f067aa0ba902b7";

/// The emulator, with request authentication off so a test signs nothing.
///
/// The one memory container whose path DOES resolve is created first, through the shared
/// inventory the CONTROL plane's `CreateMemory` writes; the ghost ids below are created
/// nowhere, which is what makes them ghosts.
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

/// The resource label a row expects the refusal to name.
///
/// Spelled out here on purpose: the expectation must be independent of the emulator's
/// own `state::NotFoundResource`, or the assertion would only re-state whatever string
/// the implementation happens to carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    Memory,
    Session,
    Event,
    Evaluator,
}

impl Resource {
    fn label(self) -> &'static str {
        match self {
            Self::Memory => "Memory",
            Self::Session => "Session",
            Self::Event => "Event",
            Self::Evaluator => "Evaluator",
        }
    }
}

/// One addressed call, with every identifier it names.
#[derive(Debug, Clone)]
enum Call {
    /// `POST /memories/{memoryId}/actor/{actorId}/sessions`
    ListSessions { memory: String, actor: String },
    /// `POST /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}`
    ListEvents {
        memory: String,
        actor: String,
        session: String,
    },
    /// `GET /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}`
    GetEvent {
        memory: String,
        actor: String,
        session: String,
        event: String,
    },
    /// `DELETE /memories/{memoryId}/actor/{actorId}/sessions/{sessionId}/events/{eventId}`
    DeleteEvent {
        memory: String,
        actor: String,
        session: String,
        event: String,
    },
    /// `GET /code-interpreters/{codeInterpreterIdentifier}/sessions/get?sessionId=…`
    GetCodeInterpreterSession {
        interpreter: String,
        session: String,
    },
    /// `POST /evaluations/evaluate/{evaluatorId}`
    Evaluate { evaluator: String },
}

impl Call {
    /// The name of the operation this call reaches, for an assertion message.
    fn operation(&self) -> &'static str {
        match self {
            Self::ListSessions { .. } => "ListSessions",
            Self::ListEvents { .. } => "ListEvents",
            Self::GetEvent { .. } => "GetEvent",
            Self::DeleteEvent { .. } => "DeleteEvent",
            Self::GetCodeInterpreterSession { .. } => "GetCodeInterpreterSession",
            Self::Evaluate { .. } => "Evaluate",
        }
    }

    /// Every identifier this call names, so a row can assert which of them the refusal
    /// is allowed to carry.
    fn identifiers(&self) -> Vec<String> {
        match self {
            Self::ListSessions { memory, actor } => vec![memory.clone(), actor.clone()],
            Self::ListEvents {
                memory,
                actor,
                session,
            } => vec![memory.clone(), actor.clone(), session.clone()],
            Self::GetEvent {
                memory,
                actor,
                session,
                event,
            }
            | Self::DeleteEvent {
                memory,
                actor,
                session,
                event,
            } => vec![
                memory.clone(),
                actor.clone(),
                session.clone(),
                event.clone(),
            ],
            Self::GetCodeInterpreterSession {
                interpreter,
                session,
            } => vec![interpreter.clone(), session.clone()],
            Self::Evaluate { evaluator } => vec![evaluator.clone()],
        }
    }

    async fn send(&self, router: &MultiServiceRouter) -> (u16, Value) {
        match self {
            Self::ListSessions { memory, actor } => {
                send(
                    router,
                    "POST",
                    &format!("/memories/{}/actor/{}/sessions", label(memory), label(actor)),
                    Some(json!({})),
                )
                .await
            }
            Self::ListEvents {
                memory,
                actor,
                session,
            } => {
                send(
                    router,
                    "POST",
                    &format!(
                        "/memories/{}/actor/{}/sessions/{}",
                        label(memory),
                        label(actor),
                        label(session)
                    ),
                    Some(json!({})),
                )
                .await
            }
            Self::GetEvent {
                memory,
                actor,
                session,
                event,
            } => {
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
            Self::DeleteEvent {
                memory,
                actor,
                session,
                event,
            } => {
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
            Self::GetCodeInterpreterSession {
                interpreter,
                session,
            } => {
                send(
                    router,
                    "GET",
                    &format!(
                        "/code-interpreters/{}/sessions/get?sessionId={}",
                        label(interpreter),
                        label(session)
                    ),
                    None,
                )
                .await
            }
            Self::Evaluate { evaluator } => {
                send(
                    router,
                    "POST",
                    &format!("/evaluations/evaluate/{}", label(evaluator)),
                    Some(evaluation_request()),
                )
                .await
            }
        }
    }
}

/// The smallest `Evaluate` body this service accepts: one span, carrying the session it
/// belongs to under the `session.id` attribute.
fn evaluation_request() -> Value {
    json!({
        "evaluationInput": {
            "sessionSpans": [{
                "name": "agent.turn",
                "spanId": SPAN,
                "traceId": TRACE,
                "startTimeUnixNano": "1",
                "attributes": [
                    {"key": "session.id", "value": {"stringValue": "session-01"}},
                    {"key": "gen_ai.completion", "value": {"stringValue": "an answer"}},
                ],
            }],
        },
    })
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// Store one event, so the suite has a memory container, an actor, a session and an
/// event that DO resolve, and return the `eventId` it minted.
async fn store_one_event(router: &MultiServiceRouter) -> String {
    let (status, created) = send(
        router,
        "POST",
        &format!("/memories/{}/events", label(MEMORY)),
        Some(json!({
            "actorId": ACTOR,
            "sessionId": SESSION,
            "eventTimestamp": 1_700_000_000,
            "payload": [{"conversational": {"role": "USER", "content": {"text": "hello"}}}],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture event must be created, got {status} with {created:?}"
    );
    created["event"]["eventId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start one code interpreter session, so the tools plane too has a record that
/// resolves, and return the `sessionId` it minted.
async fn start_one_code_interpreter_session(router: &MultiServiceRouter) -> String {
    let (status, started) = send(
        router,
        "PUT",
        &format!("/code-interpreters/{}/sessions/start", label(INTERPRETER)),
        Some(json!({"name": "probe", "sessionTimeoutSeconds": 900})),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture code interpreter session must start, got {status} with {started:?}"
    );
    started["sessionId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every refusal names the resource that did not resolve, with that resource's own
/// identifier as the request spelled it, and names nothing that did resolve.
#[tokio::test]
async fn a_lookup_that_misses_is_refused_with_the_resources_own_label_and_identifier() {
    let router = emulator();
    let event = store_one_event(&router).await;
    let code_session = start_one_code_interpreter_session(&router).await;

    // Rows: the operation that refuses × which step of the path is missing × the form the
    // identifier arrived in.
    let cases: Vec<(&str, Call, Resource, String)> = vec![
        (
            "list-sessions-unknown-memory",
            Call::ListSessions {
                memory: GHOST_MEMORY.to_string(),
                actor: ACTOR.to_string(),
            },
            Resource::Memory,
            GHOST_MEMORY.to_string(),
        ),
        (
            "list-sessions-unknown-memory-by-arn",
            Call::ListSessions {
                memory: GHOST_MEMORY_ARN.to_string(),
                actor: ACTOR.to_string(),
            },
            Resource::Memory,
            GHOST_MEMORY_ARN.to_string(),
        ),
        (
            "list-events-unknown-memory",
            Call::ListEvents {
                memory: GHOST_MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: SESSION.to_string(),
            },
            Resource::Memory,
            GHOST_MEMORY.to_string(),
        ),
        (
            "list-events-unknown-memory-and-inner-ids",
            Call::ListEvents {
                memory: GHOST_MEMORY.to_string(),
                actor: GHOST_ACTOR.to_string(),
                session: GHOST_SESSION.to_string(),
            },
            Resource::Memory,
            GHOST_MEMORY.to_string(),
        ),
        (
            "list-events-unknown-session",
            Call::ListEvents {
                memory: MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: GHOST_SESSION.to_string(),
            },
            Resource::Session,
            GHOST_SESSION.to_string(),
        ),
        (
            "list-events-unknown-actor",
            Call::ListEvents {
                memory: MEMORY.to_string(),
                actor: GHOST_ACTOR.to_string(),
                session: SESSION.to_string(),
            },
            Resource::Session,
            SESSION.to_string(),
        ),
        (
            "get-event-unknown-memory",
            Call::GetEvent {
                memory: GHOST_MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: SESSION.to_string(),
                event: event.clone(),
            },
            Resource::Memory,
            GHOST_MEMORY.to_string(),
        ),
        (
            "get-event-unknown-memory-by-arn",
            Call::GetEvent {
                memory: GHOST_MEMORY_ARN.to_string(),
                actor: ACTOR.to_string(),
                session: SESSION.to_string(),
                event: event.clone(),
            },
            Resource::Memory,
            GHOST_MEMORY_ARN.to_string(),
        ),
        (
            "get-event-unknown-session",
            Call::GetEvent {
                memory: MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: GHOST_SESSION.to_string(),
                event: event.clone(),
            },
            Resource::Session,
            GHOST_SESSION.to_string(),
        ),
        (
            "get-event-unknown-event",
            Call::GetEvent {
                memory: MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: SESSION.to_string(),
                event: GHOST_EVENT.to_string(),
            },
            Resource::Event,
            GHOST_EVENT.to_string(),
        ),
        (
            "delete-event-unknown-memory",
            Call::DeleteEvent {
                memory: GHOST_MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: SESSION.to_string(),
                event: event.clone(),
            },
            Resource::Memory,
            GHOST_MEMORY.to_string(),
        ),
        (
            "delete-event-unknown-session",
            Call::DeleteEvent {
                memory: MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: GHOST_SESSION.to_string(),
                event: event.clone(),
            },
            Resource::Session,
            GHOST_SESSION.to_string(),
        ),
        (
            "delete-event-unknown-event",
            Call::DeleteEvent {
                memory: MEMORY.to_string(),
                actor: ACTOR.to_string(),
                session: SESSION.to_string(),
                event: GHOST_EVENT.to_string(),
            },
            Resource::Event,
            GHOST_EVENT.to_string(),
        ),
        (
            "code-interpreter-unknown-session",
            Call::GetCodeInterpreterSession {
                interpreter: INTERPRETER.to_string(),
                session: GHOST_CODE_SESSION.to_string(),
            },
            Resource::Session,
            GHOST_CODE_SESSION.to_string(),
        ),
        (
            "code-interpreter-session-of-another-interpreter",
            Call::GetCodeInterpreterSession {
                interpreter: OTHER_INTERPRETER.to_string(),
                session: code_session.clone(),
            },
            Resource::Session,
            code_session.clone(),
        ),
        (
            "unknown-evaluator",
            Call::Evaluate {
                evaluator: GHOST_EVALUATOR.to_string(),
            },
            Resource::Evaluator,
            GHOST_EVALUATOR.to_string(),
        ),
    ];

    for (case, call, resource, identifier) in cases {
        let operation = call.operation();
        let (status, body) = call.send(&router).await;
        assert_eq!(
            status, 404,
            "case {case:?} on {operation}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            RESOURCE_NOT_FOUND,
            "case {case:?} on {operation}: expected {RESOURCE_NOT_FOUND}, got {body:?}"
        );
        let expected = format!("{} not found: {identifier}", resource.label());
        assert_eq!(
            message(&body),
            expected,
            "case {case:?} on {operation}: expected message {expected:?}, got {:?}",
            message(&body)
        );
        // The message carries the missing resource's identifier and NO other identifier
        // the request named: an id that resolved is not disclosed by this refusal.
        for other in call.identifiers() {
            if other == identifier {
                continue;
            }
            assert!(
                !message(&body).contains(&other),
                "case {case:?} on {operation}: the message must not name {other:?}, got {:?}",
                message(&body)
            );
        }
    }

    // Nothing above wrote: the stored event is still readable, and the fixture session is
    // still the only one the actor has.
    let (status, read) = Call::GetEvent {
        memory: MEMORY.to_string(),
        actor: ACTOR.to_string(),
        session: SESSION.to_string(),
        event: event.clone(),
    }
    .send(&router)
    .await;
    assert_eq!(
        status, 200,
        "after every refusal the stored event must still be readable, got {status} with {read:?}"
    );
    assert_eq!(
        read["event"]["eventId"].as_str(),
        Some(event.as_str()),
        "after every refusal GetEvent must report the stored event, got {read:?}"
    );
}

/// An actor is not a resource addressable on its own in the Memory event plane: a
/// listing for an actor with nothing stored is an empty page, so a request whose actor is
/// unknown is refused for the SESSION it names, never for the actor.
#[tokio::test]
async fn an_unknown_actor_is_an_empty_listing_not_a_refusal() {
    let router = emulator();
    let _event = store_one_event(&router).await;

    for (case, memory) in [
        ("bare-id", MEMORY),
        (
            "arn-spelling",
            "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/demo-memory-0123456789",
        ),
    ] {
        let (status, body) = Call::ListSessions {
            memory: memory.to_string(),
            actor: GHOST_ACTOR.to_string(),
        }
        .send(&router)
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an unknown actor in a known memory is an empty page, got {status} with {body:?}"
        );
        assert_eq!(
            body["sessionSummaries"],
            json!([]),
            "case {case:?}: expected no session summaries, got {body:?}"
        );
    }

    // And the evaluator family that IS provided still evaluates, so the refusal above is
    // about the identifier and not about the operation.
    let (status, body) = Call::Evaluate {
        evaluator: EVALUATOR.to_string(),
    }
    .send(&router)
    .await;
    assert_eq!(
        status, 200,
        "a provided evaluator must still evaluate, got {status} with {body:?}"
    );
}
