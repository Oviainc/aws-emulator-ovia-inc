// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MODELLED REFUSALS this service answers with: the HTTP
//! status, the `x-amzn-errortype` code and the message text.
//!
//! The rule under test: each refusal condition has ONE fixed sentence, the same for
//! every request that meets it, quoting nothing from the request — the requested label
//! travels in `ResourceNotFoundException`'s own `resourceName` member instead. A message
//! that interpolates the requested id is a message this service never sends.
//!
//! The test is a TABLE over refusal conditions and over the forms a request can address
//! a resource by (a bare UUID, this account's ARN, another account's ARN for a live id,
//! another partition's), because the rule generalises over all of them: one condition,
//! one answer.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The service's fixed sentence for each refusal condition, WRITTEN OUT here rather
/// than imported from `state.rs`: a test that imported the handler's own constant would
/// re-assert whatever the code says and could not fail if the wording changed. Each is
/// the text the recorded wire answers carry for its condition.
const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";
const MESSAGE_SESSION_NOT_FOUND: &str = "Session does not exist.";
const MESSAGE_NAME_IN_USE: &str = "Name is already in use";
const MESSAGE_UNAUTHORIZED: &str = "Unauthorized";

/// The region every request is signed for.
const REGION: &str = "us-west-2";

/// A UUID no create ever mints.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn string(&self, pointer: &str) -> Option<String> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// An emulator with just this service mounted on a fresh engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("wisdom.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }
}

/// A `UuidOrArn` label as it travels in a URL: an ARN's slashes are percent-encoded, or
/// they would be path separators naming another route.
fn encode_label(label: &str) -> String {
    label.replace('/', "%2F")
}

/// This account's assistant ARN for `assistant_id`.
fn assistant_arn(assistant_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{REGION}:{account}:assistant/{assistant_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `CreateAIAgent` body the model accepts: `name`, `type`, `configuration` and
/// `visibilityStatus` are required, and every member of the `MANUAL_SEARCH`
/// configuration is optional
/// (`smithy_query(query="operation",operation="CreateAIAgent")`).
fn ai_agent_body() -> Value {
    json!({
        "name": "an-agent",
        "type": "MANUAL_SEARCH",
        "visibilityStatus": "SAVED",
        "configuration": { "manualSearchAIAgentConfiguration": {} },
    })
}

/// What one row of the table expects on the wire.
#[derive(Debug)]
struct Expected {
    status: u16,
    error_type: &'static str,
    message: &'static str,
}

/// Each condition answers one status, one code and one sentence, whichever label form
/// the request addressed the resource by — and the sentence never quotes the request.
#[tokio::test]
async fn every_refusal_answers_its_own_status_code_and_fixed_text() {
    let emulator = Emulator::new();

    // A live assistant, a live knowledge base and a live session, so the rows below can
    // address a real resource as well as an absent one.
    let live_assistant = emulator
        .send(
            "POST",
            "/assistants",
            Some(json!({ "name": "live", "type": "AGENT" })),
        )
        .await;
    let assistant_id = live_assistant
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", live_assistant.body));
    let live_kb = emulator
        .send(
            "POST",
            "/knowledgeBases",
            Some(json!({ "name": "live-kb", "knowledgeBaseType": "CUSTOM" })),
        )
        .await;
    assert_eq!(
        live_kb.status, 200,
        "setup: expected the knowledge base to be created, got {} with body {}",
        live_kb.status, live_kb.body
    );

    // The label forms a request can name a resource by: each must answer the same way.
    let absent_labels = [
        ("bare uuid", ABSENT_UUID.to_string()),
        ("this account's arn", assistant_arn(ABSENT_UUID)),
        (
            "another account's arn for a live id",
            format!("arn:aws:wisdom:{REGION}:123456789012:assistant/{assistant_id}"),
        ),
        (
            "another partition's arn for a live id",
            format!(
                "arn:aws-cn:wisdom:{REGION}:{account}:assistant/{assistant_id}",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];

    let not_found_assistant = Expected {
        status: 404,
        error_type: "ResourceNotFoundException",
        message: MESSAGE_ASSISTANT_NOT_FOUND,
    };

    for (form, label) in &absent_labels {
        let encoded = encode_label(label);
        let rows: Vec<(String, Reply, &Expected)> = vec![
            (
                format!("GetAssistant/{form}"),
                emulator
                    .send("GET", &format!("/assistants/{encoded}"), None)
                    .await,
                &not_found_assistant,
            ),
            (
                format!("DeleteAssistant/{form}"),
                emulator
                    .send("DELETE", &format!("/assistants/{encoded}"), None)
                    .await,
                &not_found_assistant,
            ),
            (
                format!("GetSession under an absent assistant/{form}"),
                emulator
                    .send(
                        "GET",
                        &format!("/assistants/{encoded}/sessions/{ABSENT_UUID}"),
                        None,
                    )
                    .await,
                &not_found_assistant,
            ),
            (
                // An AI agent is created inside an assistant: an absent parent is this
                // client error, not the 501 the unwritten success path answers.
                format!("CreateAIAgent under an absent assistant/{form}"),
                emulator
                    .send(
                        "POST",
                        &format!("/assistants/{encoded}/aiagents"),
                        Some(ai_agent_body()),
                    )
                    .await,
                &not_found_assistant,
            ),
            (
                // The session create path authorizes the caller against the parent
                // assistant before resolving it, so a parent it cannot resolve is the
                // modelled `UnauthorizedException`.
                format!("CreateSession under an absent assistant/{form}"),
                emulator
                    .send(
                        "POST",
                        &format!("/assistants/{encoded}/sessions"),
                        Some(json!({ "name": "orphan" })),
                    )
                    .await,
                &Expected {
                    status: 401,
                    error_type: "UnauthorizedException",
                    message: MESSAGE_UNAUTHORIZED,
                },
            ),
            (
                format!("GetKnowledgeBase/{form}"),
                emulator
                    .send("GET", &format!("/knowledgeBases/{encoded}"), None)
                    .await,
                &Expected {
                    status: 404,
                    error_type: "ResourceNotFoundException",
                    message: MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
                },
            ),
        ];

        for (case, reply, expected) in rows {
            assert_refused(&case, &reply, expected);
            assert_quotes_nothing(&case, &reply, label);
        }
    }

    // A session id its own parent does not hold is the session's own condition.
    let session_under_live_parent = emulator
        .send(
            "GET",
            &format!("/assistants/{assistant_id}/sessions/{ABSENT_UUID}"),
            None,
        )
        .await;
    assert_refused(
        "GetSession with an absent session under a live parent",
        &session_under_live_parent,
        &Expected {
            status: 404,
            error_type: "ResourceNotFoundException",
            message: MESSAGE_SESSION_NOT_FOUND,
        },
    );
    assert_quotes_nothing(
        "GetSession with an absent session under a live parent",
        &session_under_live_parent,
        ABSENT_UUID,
    );

    // A name a live resource already holds is the conflict, on both creates.
    let duplicate_assistant = emulator
        .send(
            "POST",
            "/assistants",
            Some(json!({ "name": "live", "type": "AGENT" })),
        )
        .await;
    assert_refused(
        "CreateAssistant with a name in use",
        &duplicate_assistant,
        &Expected {
            status: 409,
            error_type: "ConflictException",
            message: MESSAGE_NAME_IN_USE,
        },
    );
    assert_quotes_nothing(
        "CreateAssistant with a name in use",
        &duplicate_assistant,
        "live",
    );

    let duplicate_kb = emulator
        .send(
            "POST",
            "/knowledgeBases",
            Some(json!({ "name": "live-kb", "knowledgeBaseType": "CUSTOM" })),
        )
        .await;
    assert_refused(
        "CreateKnowledgeBase with a name in use",
        &duplicate_kb,
        &Expected {
            status: 409,
            error_type: "ConflictException",
            message: MESSAGE_NAME_IN_USE,
        },
    );
    assert_quotes_nothing(
        "CreateKnowledgeBase with a name in use",
        &duplicate_kb,
        "live-kb",
    );
}

/// A not-found answer names the label the request used in `resourceName`, which is the
/// member the model provides for it — so the id is still reported, just not in the
/// sentence.
#[tokio::test]
async fn a_not_found_answer_names_the_addressed_label_in_resource_name() {
    let emulator = Emulator::new();

    for (case, label) in [
        ("bare uuid", ABSENT_UUID.to_string()),
        ("this account's arn", assistant_arn(ABSENT_UUID)),
    ] {
        let reply = emulator
            .send(
                "GET",
                &format!("/assistants/{}", encode_label(&label)),
                None,
            )
            .await;
        assert_eq!(
            reply.string("/resourceName").as_deref(),
            Some(label.as_str()),
            "case {case:?}: expected resourceName to name the addressed label, got {:?}",
            reply.string("/resourceName")
        );
    }
}

/// `CreateAIAgent`'s success path is still scaffolded: a create whose parent assistant
/// DOES resolve answers 501, so the refusal above is the client error it is and not a
/// side effect of the operation being unwritten.
#[tokio::test]
async fn create_ai_agent_under_a_live_assistant_is_still_scaffolded() {
    let emulator = Emulator::new();
    let created = emulator
        .send(
            "POST",
            "/assistants",
            Some(json!({ "name": "agent-parent", "type": "AGENT" })),
        )
        .await;
    let assistant_id = created
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", created.body));

    let reply = emulator
        .send(
            "POST",
            &format!("/assistants/{assistant_id}/aiagents"),
            Some(ai_agent_body()),
        )
        .await;
    assert_eq!(
        reply.status, 501,
        "case {:?}: expected HTTP 501 for the unwritten success path, got {} with body {}",
        "live parent", reply.status, reply.body
    );
}

/// Assert one refused reply reports exactly `expected`.
fn assert_refused(case: &str, reply: &Reply, expected: &Expected) {
    assert_eq!(
        reply.status, expected.status,
        "case {case:?}: expected HTTP {}, got {} with body {}",
        expected.status, reply.status, reply.body
    );
    assert!(
        reply.error_type.contains(expected.error_type),
        "case {case:?}: expected x-amzn-errortype {}, got {:?} with body {}",
        expected.error_type,
        reply.error_type,
        reply.body
    );
    let message = reply.string("/message");
    assert_eq!(
        message.as_deref(),
        Some(expected.message),
        "case {case:?}: expected the fixed text {:?}, got {message:?}",
        expected.message
    );
}

/// Assert the message quotes nothing the request carried.
fn assert_quotes_nothing(case: &str, reply: &Reply, from_request: &str) {
    let message = reply.string("/message").unwrap_or_default();
    assert!(
        !message.contains(from_request),
        "case {case:?}: the message quotes {from_request:?} from the request: {message:?}"
    );
}
