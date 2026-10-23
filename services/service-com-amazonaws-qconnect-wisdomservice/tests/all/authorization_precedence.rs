// SPDX-License-Identifier: CC0-1.0

//! Where this endpoint makes an AUTHORIZATION decision, that decision is reached BEFORE
//! the request's members are validated against the model's constraints.
//!
//! The rule under test: a caller this service cannot authorize for the resource a
//! request's path names is refused with the operation's modelled authorization error —
//! HTTP 401 `UnauthorizedException` — whatever else the same request violates (a path
//! label outside its `@pattern`, a body member outside its `@pattern` or `@length`, an
//! absent `@required` member). The answer therefore does not depend on which of the two
//! faults the request carries, so a client refused for permissions is never told its
//! input is malformed instead. `CreateSession` is the one operation this service decides
//! authorization for: it answers the modelled `UnauthorizedException` (HTTP 401) for a
//! parent assistant the account does not hold
//! (`smithy_query(query="operation",operation="CreateSession")` lists it beside
//! `ValidationException`), while every other operation resolves its resource instead and
//! answers `ResourceNotFoundException` — a resource decision, which stays BEHIND
//! validation.
//!
//! The table's rows differ along what the rule generalises over: the FORM of the
//! unauthorized label (malformed in several ways, well-formed but naming nothing, an ARN
//! of this account, of another account and of another partition) crossed with WHICH
//! constraint the rest of the request breaks (none, a `@pattern`, a `@length`, an absent
//! `@required` member). Every row must answer the one authorization refusal.
//!
//! The controls pin the other side of the rule: for a caller that IS authorized the
//! modelled `ValidationException` still answers input the model refuses, a valid create
//! still succeeds and reads back, and an operation whose refusal is a RESOURCE decision
//! (`GetAssistant`) keeps validation in front of it.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts. The suite carries its own request harness, as each suite
//! in this crate does: sharing one would make a neighbouring suite's helpers this
//! suite's contract.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// A UUID the modelled `UuidOrArn` pattern admits and no create ever mints.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The modelled error codes under test, written out rather than imported: a test that
/// imported the handler's own constants could not fail if they changed.
const UNAUTHORIZED: &str = "UnauthorizedException";
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    /// The restJson1 error code: the `x-amzn-errortype` header, which is what a
    /// restJson1 client resolves the error shape from, falling back to the body's
    /// `__type` for a response that carries no header.
    fn code(&self) -> String {
        if !self.error_type.is_empty() {
            return self.error_type.clone();
        }
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

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
             SignedHeaders=host, Signature=not-verified-in-this-suite"
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

/// The `CreateSession` route for `label`: `POST /assistants/{assistantId}/sessions`
/// (`smithy_query(query="operation",operation="CreateSession")`).
fn create_session_uri(label: &str) -> String {
    format!("/assistants/{}/sessions", encode_label(label))
}

/// Every form an `assistantId` label can take that this account is not authorized for,
/// whether or not the modelled `UuidOrArn` pattern would admit it.
///
/// The pattern is
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$|^arn:[a-z-]*?:wisdom:[a-z0-9-]*?:[0-9]{12}:[a-z-]*?/<uuid>(?:/<uuid>){0,2}$`
/// (`smithy_query(query="shape",shape="UuidOrArn")`), so the first five rows are
/// labels it REFUSES and the last three are labels it admits that name nothing here.
fn unauthorized_labels() -> Vec<(&'static str, String)> {
    vec![
        // Outside the modelled pattern — the class the authorization decision must get
        // in front of.
        ("not a uuid at all", "not-a-valid-id".to_string()),
        ("a single character", "x".to_string()),
        (
            "a uuid in the wrong case",
            "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA".to_string(),
        ),
        (
            "a uuid missing a group",
            "11111111-1111-1111-111111111111".to_string(),
        ),
        (
            "an arn of this service whose resource id is not a uuid",
            format!(
                "arn:aws:wisdom:{REGION}:{account}:assistant/not-a-uuid",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        // Admitted by the pattern, and naming no assistant this account holds.
        ("a well-formed absent uuid", ABSENT_UUID.to_string()),
        (
            "this account's arn for an absent uuid",
            format!(
                "arn:aws:wisdom:{REGION}:{account}:assistant/{ABSENT_UUID}",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "another account's arn",
            format!("arn:aws:wisdom:{REGION}:123456789012:assistant/{ABSENT_UUID}"),
        ),
        (
            "another partition's arn",
            format!(
                "arn:aws-cn:wisdom:{REGION}:{account}:assistant/{ABSENT_UUID}",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ]
}

/// Request bodies spanning what else a `CreateSession` can get wrong, from nothing at
/// all to each constraint class the model declares for it
/// (`name` **required**, `Name` len 1..255 pattern `^[a-zA-Z0-9\s_.,-]+`; `description`
/// len 1..255 with the same pattern; `contactArn` len 1..2048 pattern
/// `^arn:[a-z-]+?:[a-z-]+?:[a-z0-9-]*?:([0-9]{12})?:[a-zA-Z0-9-:/]+$`;
/// `clientToken` len 1..4096).
fn create_session_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("a body the model accepts", json!({ "name": "a-session" })),
        ("the required name absent", json!({})),
        (
            "a name outside the Name pattern",
            json!({ "name": "!!!" }),
        ),
        (
            "a description under the Description minimum length",
            json!({ "name": "a-session", "description": "" }),
        ),
        (
            "a contactArn outside the GenericArn pattern",
            json!({ "name": "a-session", "contactArn": "not-an-arn" }),
        ),
        (
            "a clientToken under the ClientToken minimum length",
            json!({ "name": "a-session", "clientToken": "" }),
        ),
    ]
}

/// The authorization refusal answers every unauthorized label, crossed with every way
/// the rest of the request can break the model's constraints.
#[tokio::test]
async fn an_unauthorized_create_session_is_refused_before_its_input_is_validated() {
    let emulator = Emulator::new();

    for (label_case, label) in unauthorized_labels() {
        for (body_case, body) in create_session_bodies() {
            let case = format!("{label_case} + {body_case}");
            let reply = emulator
                .send("POST", &create_session_uri(&label), Some(body))
                .await;
            assert_eq!(
                (reply.status, reply.code().as_str()),
                (401, UNAUTHORIZED),
                "case {case:?}: expected HTTP 401 {UNAUTHORIZED}, got {} {:?} ({})",
                reply.status,
                reply.code(),
                reply.body
            );
            let message = reply.string("/message").unwrap_or_default();
            assert!(
                !message.contains(&label) && !message.contains("constraint"),
                "case {case:?}: the authorization refusal must not report the input as \
                 malformed, got {message:?}"
            );
        }
    }
}

/// For a caller that IS authorized, validation still answers: the modelled
/// `ValidationException` refuses input the model rejects, and a request the model
/// accepts creates a session that reads back through `GetSession`.
#[tokio::test]
async fn an_authorized_create_session_still_validates_its_input() {
    let emulator = Emulator::new();

    let created = emulator
        .send(
            "POST",
            "/assistants",
            Some(json!({ "name": "precedence-parent", "type": "AGENT" })),
        )
        .await;
    let assistant_id = created
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("setup: no assistantId in {}", created.body));

    // Every body the model refuses is refused with the modelled validation error now
    // that the caller is authorized for the parent — the same bodies that answered 401
    // above.
    for (body_case, body) in create_session_bodies()
        .into_iter()
        .filter(|(case, _)| *case != "a body the model accepts")
    {
        let reply = emulator
            .send("POST", &create_session_uri(&assistant_id), Some(body))
            .await;
        assert_eq!(
            (reply.status, reply.code().as_str()),
            (400, VALIDATION),
            "case {body_case:?} under an authorized parent: expected HTTP 400 \
             {VALIDATION}, got {} {:?} ({})",
            reply.status,
            reply.code(),
            reply.body
        );
    }

    // And the body the model accepts creates a session that is persisted.
    let session = emulator
        .send(
            "POST",
            &create_session_uri(&assistant_id),
            Some(json!({ "name": "a-session" })),
        )
        .await;
    assert_eq!(
        session.status, 200,
        "case \"a valid create under an authorized parent\": expected HTTP 200, got {} \
         ({})",
        session.status, session.body
    );
    let session_id = session
        .string("/session/sessionId")
        .unwrap_or_else(|| panic!("no sessionId in {}", session.body));

    let read_back = emulator
        .send(
            "GET",
            &format!("/assistants/{assistant_id}/sessions/{session_id}"),
            None,
        )
        .await;
    assert_eq!(
        read_back.status, 200,
        "case \"GetSession after CreateSession\": expected HTTP 200, got {} ({})",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.body.pointer("/session"),
        session.body.pointer("/session"),
        "case \"GetSession after CreateSession\": the session read back must be the one \
         created, got {} for {}",
        read_back.body,
        session.body
    );
}

/// The precedence belongs to the AUTHORIZATION class alone. `GetAssistant` refuses an
/// assistant it cannot resolve with `ResourceNotFoundException` — a resource decision —
/// so validation still runs in front of it: a label outside the modelled pattern is the
/// modelled `ValidationException`, and only a label the pattern admits reaches the
/// not-found answer.
#[tokio::test]
async fn an_operation_whose_refusal_is_a_resource_decision_keeps_validation_in_front() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "GetAssistant with a label outside the UuidOrArn pattern",
            "not-a-valid-id".to_string(),
            400,
            VALIDATION,
        ),
        (
            "GetAssistant with a single-character label",
            "x".to_string(),
            400,
            VALIDATION,
        ),
        (
            "GetAssistant with a well-formed absent uuid",
            ABSENT_UUID.to_string(),
            404,
            NOT_FOUND,
        ),
        (
            "GetAssistant with this account's arn for an absent uuid",
            format!(
                "arn:aws:wisdom:{REGION}:{account}:assistant/{ABSENT_UUID}",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
            404,
            NOT_FOUND,
        ),
    ];

    for (case, label, expected_status, expected_code) in rows {
        let reply = emulator
            .send("GET", &format!("/assistants/{}", encode_label(&label)), None)
            .await;
        assert_eq!(
            (reply.status, reply.code().as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {} {:?} \
             ({})",
            reply.status,
            reply.code(),
            reply.body
        );
    }
}
