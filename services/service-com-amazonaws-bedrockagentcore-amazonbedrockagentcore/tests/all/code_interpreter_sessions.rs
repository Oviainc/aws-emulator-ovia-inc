// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore code interpreter session lifecycle:
//! `StartCodeInterpreterSession` and `GetCodeInterpreterSession`.
//!
//! The rule under test: a start MINTS a `sessionId` matching its modelled pattern,
//! persists the configuration the request asked for (name, time-to-live, certificates,
//! file system configurations) under the named code interpreter together with the
//! instant it was created, and reports that stored record back; a replayed
//! `clientToken` answers with the session that token already started instead of
//! starting a second one; and a read reports the stored record with the status the
//! clock implies — `READY` until the stored time-to-live has run out, `TERMINATED`
//! from then on — while a session this emulator does not hold is a 404.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so the wire shape (routes, status codes, `restJson1` bodies, modelled
//! error codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::{DateTime, Format};
use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

use super::harness::{create_code_interpreter, label};

/// The system code interpreter AWS ships; `codeInterpreterIdentifier` is modelled as a
/// plain string, so any identifier a caller names addresses its own session set.
const SYSTEM_INTERPRETER: &str = "aws.codeinterpreter.v1";
/// A second, customer-created code interpreter, so a case can show that sessions are
/// kept per code interpreter.
const CUSTOM_INTERPRETER: &str = "demo_interpreter-1a2b3c4d5e";

/// The emulator, with request authentication off so a test signs nothing, plus the
/// shared engine so a case can age a stored session's clock without sleeping.
///
/// The customer-created interpreter this suite starts sessions on is created first,
/// through the shared inventory the CONTROL plane's `CreateCodeInterpreter` writes: a
/// session cannot be started on an interpreter that was never created. The interpreter
/// the service provides needs no such call — nothing creates it and nothing deletes it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    create_code_interpreter(&engine, CUSTOM_INTERPRETER);
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

/// `StartCodeInterpreterSession`: `PUT /code-interpreters/{codeInterpreterIdentifier}/sessions/start`.
async fn start_session(
    router: &MultiServiceRouter,
    interpreter: &str,
    body: Value,
) -> (u16, Value) {
    send(
        router,
        "PUT",
        &format!("/code-interpreters/{}/sessions/start", label(interpreter)),
        Some(body),
    )
    .await
}

/// `GetCodeInterpreterSession`: `GET /code-interpreters/{codeInterpreterIdentifier}/sessions/get?sessionId=…`.
async fn get_session(
    router: &MultiServiceRouter,
    interpreter: &str,
    session_id: &str,
) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!(
            "/code-interpreters/{}/sessions/get?sessionId={}",
            label(interpreter),
            label(session_id)
        ),
        None,
    )
    .await
}

/// Move a stored session's creation instant `by` further into the past, so a case can
/// reach the far side of a time-to-live without sleeping for it. This is the test
/// reaching into the store the handlers share, not an emulator behaviour.
fn age_session(
    engine: &engine::InMemoryEngine,
    interpreter: &str,
    session_id: &str,
    by: Duration,
) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let session = store
        .code_interpreters
        .get_mut(interpreter)
        .and_then(|held| held.sessions.get_mut(session_id))
        .expect("the session that was just started");
    session.created_at -= by;
}

/// How many sessions this emulator holds for one code interpreter — the check that a
/// replayed `clientToken` started nothing new.
fn session_count(engine: &engine::InMemoryEngine, interpreter: &str) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store
        .code_interpreters
        .get(interpreter)
        .map_or(0, |held| held.sessions.len())
}

/// `CodeInterpreterSessionId`'s modelled pattern, `^[0-9a-zA-Z]{1,40}$`, checked
/// without a regex dependency.
fn is_modelled_session_id(id: &str) -> bool {
    (1..=40).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A `DateTimestamp` member carries `@timestampFormat("date-time")`, so on this
/// service's restJson1 wire it is an RFC 3339 string rather than an epoch number
/// (source: smithy_query query="shape" shape="DateTimestamp").
fn assert_date_time(case: &str, member_name: &str, value: &Value) {
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected {member_name} as a date-time string, got {value:?}")
    });
    assert!(
        DateTime::from_str(text, Format::DateTime).is_ok(),
        "case {case:?}: expected {member_name} to parse as RFC 3339, got {text:?}"
    );
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    member(body, name).as_str().unwrap_or("")
}

/// One certificate, as `Certificate$location` models it: the `secretsManager` member of
/// the `CertificateLocation` union (source: smithy_query query="shape"
/// shape="CertificateLocation").
fn certificate(secret: &str) -> Value {
    json!({"location": {"secretsManager": {"secretArn": secret}}})
}

/// A started session is reported from what was stored, for every combination of the
/// optional members `StartCodeInterpreterSessionRequest` models and both published
/// bounds of its time-to-live — and a `GetCodeInterpreterSession` afterwards reports
/// the same identity with the configuration that was asked for.
#[tokio::test]
async fn a_started_session_round_trips_through_get_code_interpreter_session() {
    let (router, _engine) = emulator();

    // Rows differ along the code interpreter, whether `name` was supplied and how long
    // it is, the time-to-live (omitted so the modelled default applies, both published
    // bounds of `CodeInterpreterSessionTimeout` 1..28800, and the recommended minimum),
    // and which of `certificates` / `filesystemConfigurations` were supplied.
    // `ToolsFileSystemConfiguration` members: s3FilesConfiguration | efsConfiguration
    // (source: smithy_query query="shape" shape="ToolsFileSystemConfiguration").
    let cases: Vec<(&str, &str, Value, i64)> = vec![
        (
            "no-optional-members-takes-the-modelled-default",
            SYSTEM_INTERPRETER,
            json!({}),
            900,
        ),
        (
            "named-at-the-lowest-timeout",
            SYSTEM_INTERPRETER,
            json!({"name": "a", "sessionTimeoutSeconds": 1}),
            1,
        ),
        (
            "longest-name-at-the-highest-timeout",
            CUSTOM_INTERPRETER,
            json!({"name": "n".repeat(100), "sessionTimeoutSeconds": 28_800}),
            28_800,
        ),
        (
            "recommended-minimum-timeout-with-two-certificates",
            CUSTOM_INTERPRETER,
            json!({
                "name": "analysis session",
                "sessionTimeoutSeconds": 60,
                "certificates": [
                    certificate("arn:aws:secretsmanager:us-east-1:123456789012:secret:ca-one"),
                    certificate("arn:aws:secretsmanager:us-east-1:123456789012:secret:ca-two"),
                ],
            }),
            60,
        ),
        (
            "both-filesystem-union-members-mounted",
            SYSTEM_INTERPRETER,
            json!({
                "sessionTimeoutSeconds": 1_800,
                "filesystemConfigurations": [
                    // The ARN forms `S3FilesConfiguration` and `EfsConfiguration`
                    // publish as member patterns (source: smithy_query query="shape"
                    // shape="S3FilesConfiguration,EfsConfiguration").
                    {"s3FilesConfiguration": {
                        "accessPointArn": "arn:aws:s3files:us-east-1:123456789012:file-system/fs-0123456789abcdef0/access-point/fsap-0123456789abcdef0",
                        "mountPath": "/mnt/s3",
                        "fileSystemArn": "arn:aws:s3files:us-east-1:123456789012:file-system/fs-0123456789abcdef0",
                    }},
                    {"efsConfiguration": {
                        "accessPointArn": "arn:aws:elasticfilesystem:us-east-1:123456789012:access-point/fsap-0123456789abcdef0",
                        "mountPath": "/mnt/efs",
                        "fileSystemArn": "arn:aws:elasticfilesystem:us-east-1:123456789012:file-system/fs-0123456789abcdef0",
                    }},
                ],
            }),
            1_800,
        ),
        (
            "no-filesystems-mounted-is-an-empty-list",
            CUSTOM_INTERPRETER,
            json!({"sessionTimeoutSeconds": 7_200, "filesystemConfigurations": []}),
            7_200,
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, interpreter, request, expected_timeout) in cases {
        let (status, started) = start_session(&router, interpreter, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartCodeInterpreterSession, got {status} with {started:?}"
        );
        assert_eq!(
            text(&started, "codeInterpreterIdentifier"),
            interpreter,
            "case {case:?}: expected the code interpreter the request named, got {started:?}"
        );
        let session_id = text(&started, "sessionId").to_string();
        assert!(
            is_modelled_session_id(&session_id),
            "case {case:?}: expected a sessionId matching ^[0-9a-zA-Z]{{1,40}}$, got {session_id:?}"
        );
        assert!(
            minted.insert(session_id.clone()),
            "case {case:?}: expected a sessionId no earlier start minted, got {session_id:?}"
        );
        let created_at = member(&started, "createdAt").clone();
        assert_date_time(case, "createdAt", &created_at);

        let (status, read) = get_session(&router, interpreter, &session_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetCodeInterpreterSession, got {status} with {read:?}"
        );
        assert_eq!(
            text(&read, "sessionId"),
            session_id,
            "case {case:?}: expected the started session back, got {read:?}"
        );
        assert_eq!(
            text(&read, "codeInterpreterIdentifier"),
            interpreter,
            "case {case:?}: expected the code interpreter the session was started on, got {read:?}"
        );
        assert_eq!(
            member(&read, "createdAt"),
            &created_at,
            "case {case:?}: expected the creation instant the start reported, got {read:?}"
        );
        assert_date_time(case, "createdAt (read back)", member(&read, "createdAt"));
        assert_eq!(
            member(&read, "name"),
            request.get("name").unwrap_or(&Value::Null),
            "case {case:?}: expected the name the request asked for, got {read:?}"
        );
        assert_eq!(
            member(&read, "sessionTimeoutSeconds").as_i64(),
            Some(expected_timeout),
            "case {case:?}: expected the stored time-to-live {expected_timeout}, got {read:?}"
        );
        assert_eq!(
            member(&read, "certificates"),
            request.get("certificates").unwrap_or(&Value::Null),
            "case {case:?}: expected the certificates the request asked for, got {read:?}"
        );
        assert_eq!(
            member(&read, "filesystemConfigurations"),
            request
                .get("filesystemConfigurations")
                .unwrap_or(&Value::Null),
            "case {case:?}: expected the file systems the request asked for, got {read:?}"
        );
        assert_eq!(
            text(&read, "status"),
            "READY",
            "case {case:?}: expected a session inside its time-to-live to be READY, got {read:?}"
        );

        // A read changes nothing a later caller sees.
        let (status, again) = get_session(&router, interpreter, &session_id).await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {case:?}: expected a second read to report the same session, got {status} with {again:?}"
        );
    }
}

/// A session reports `READY` until the time-to-live the request asked for has run out,
/// and `TERMINATED` from that instant on — whatever that time-to-live is.
#[tokio::test]
async fn session_status_follows_the_time_to_live_the_request_asked_for() {
    let (router, engine) = emulator();

    // `CodeInterpreterSessionTimeout` is range 1..28800 with default 900 (source:
    // smithy_query query="operation" operation="StartCodeInterpreterSession"): both
    // published bounds, the default and the documented recommended minimum.
    for timeout in [1_i64, 60, 900, 28_800] {
        let case = format!("timeout-{timeout}");
        let (status, started) =
            start_session(&router, SYSTEM_INTERPRETER, json!({"sessionTimeoutSeconds": timeout}))
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartCodeInterpreterSession, got {status} with {started:?}"
        );
        let session_id = text(&started, "sessionId").to_string();

        // One second short of the time-to-live the session is still usable.
        let timeout_secs = u64::try_from(timeout).expect("a modelled timeout is positive");
        age_session(
            &engine,
            SYSTEM_INTERPRETER,
            &session_id,
            Duration::from_secs(timeout_secs - 1),
        );
        let (status, read) = get_session(&router, SYSTEM_INTERPRETER, &session_id).await;
        assert_eq!(
            (status, text(&read, "status")),
            (200, "READY"),
            "case {case:?}: expected READY one second before the time-to-live runs out, got {status} with {read:?}"
        );

        // At the instant it runs out the session is terminated, with no call needed to
        // make it so.
        age_session(
            &engine,
            SYSTEM_INTERPRETER,
            &session_id,
            Duration::from_secs(1),
        );
        let (status, read) = get_session(&router, SYSTEM_INTERPRETER, &session_id).await;
        assert_eq!(
            (status, text(&read, "status")),
            (200, "TERMINATED"),
            "case {case:?}: expected TERMINATED once the time-to-live has run out, got {status} with {read:?}"
        );
        assert_eq!(
            member(&read, "sessionTimeoutSeconds").as_i64(),
            Some(timeout),
            "case {case:?}: expected a terminated session to still report its time-to-live, got {read:?}"
        );
    }
}

/// `clientToken` is an `@idempotencyToken` on an `@idempotent` operation: replaying one
/// answers with the session it already started, and starts nothing new. Two starts that
/// carry no token are two sessions.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_session_it_already_started() {
    let (router, engine) = emulator();

    // `ClientToken` is len 33..256, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$ (source:
    // smithy_query query="operation" operation="StartCodeInterpreterSession"). Rows
    // differ along the token's length and the body sent with the replay, which the
    // service ignores for a token it has already seen.
    let cases: Vec<(&str, String, Value, Value)> = vec![
        (
            "shortest-token-identical-replay",
            "a".repeat(33),
            json!({"name": "first", "sessionTimeoutSeconds": 120}),
            json!({"name": "first", "sessionTimeoutSeconds": 120}),
        ),
        (
            "hyphenated-token-replayed-with-a-different-body",
            "replay-token-0123456789-abcdefghij-0".to_string(),
            json!({"name": "second", "sessionTimeoutSeconds": 300}),
            json!({"name": "ignored on a replay", "sessionTimeoutSeconds": 600}),
        ),
        (
            "longest-token-replay",
            "z".repeat(256),
            json!({"sessionTimeoutSeconds": 900}),
            json!({"sessionTimeoutSeconds": 900}),
        ),
    ];

    let mut expected_sessions = 0;
    for (case, token, first_body, replay_body) in cases {
        let mut first_request = first_body;
        first_request["clientToken"] = json!(token);
        let (status, started) = start_session(&router, CUSTOM_INTERPRETER, first_request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the first start, got {status} with {started:?}"
        );
        expected_sessions += 1;

        let mut replay_request = replay_body;
        replay_request["clientToken"] = json!(token);
        let (status, replayed) = start_session(&router, CUSTOM_INTERPRETER, replay_request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the replay, got {status} with {replayed:?}"
        );
        assert_eq!(
            replayed, started,
            "case {case:?}: expected the replay to answer with the session the token already started, got {replayed:?}"
        );
        assert_eq!(
            session_count(&engine, CUSTOM_INTERPRETER),
            expected_sessions,
            "case {case:?}: expected the replay to start no second session"
        );

        // The configuration the replay carried is ignored: the stored session is still
        // the one the first call asked for.
        let session_id = text(&started, "sessionId").to_string();
        let (status, read) = get_session(&router, CUSTOM_INTERPRETER, &session_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the started session to be readable, got {status} with {read:?}"
        );
        assert_eq!(
            member(&read, "createdAt"),
            member(&started, "createdAt"),
            "case {case:?}: expected the creation instant of the first start, got {read:?}"
        );
    }

    // Two starts with no token at all are two sessions.
    let (_, first) = start_session(&router, CUSTOM_INTERPRETER, json!({})).await;
    let (_, second) = start_session(&router, CUSTOM_INTERPRETER, json!({})).await;
    assert_ne!(
        text(&first, "sessionId"),
        text(&second, "sessionId"),
        "two starts carrying no clientToken must be two sessions, got {first:?} and {second:?}"
    );
    assert_eq!(
        session_count(&engine, CUSTOM_INTERPRETER),
        expected_sessions + 2,
        "expected every token-less start to be stored as its own session"
    );
}

/// A session this emulator does not hold is a 404 `ResourceNotFoundException` — and a
/// session is addressed by the code interpreter it was started on, so the same id asked
/// for on another code interpreter is not found either.
#[tokio::test]
async fn a_session_this_emulator_does_not_hold_is_refused_with_404() {
    let (router, _engine) = emulator();

    let (_, started) = start_session(&router, SYSTEM_INTERPRETER, json!({})).await;
    let known = text(&started, "sessionId").to_string();

    let cases: Vec<(&str, &str, String)> = vec![
        (
            "unknown-session-on-a-known-code-interpreter",
            SYSTEM_INTERPRETER,
            "01abcdef0123456789".to_string(),
        ),
        (
            "known-session-on-another-code-interpreter",
            CUSTOM_INTERPRETER,
            known.clone(),
        ),
        (
            "session-on-a-code-interpreter-nothing-was-started-on",
            "aws.codeinterpreter.v2",
            known,
        ),
    ];

    for (case, interpreter, session_id) in cases {
        let (status, body) = get_session(&router, interpreter, &session_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}

/// Input outside a modelled constraint is refused with a 400 `ValidationException`,
/// never accepted and never answered with a 5xx: one case on each side of
/// `CodeInterpreterSessionTimeout`'s published range, and one on each side of the
/// length `Certificates` and `Name` publish.
#[tokio::test]
async fn input_outside_a_modelled_constraint_is_refused_with_400() {
    let (router, engine) = emulator();

    let cases: Vec<(&str, Value)> = vec![
        ("timeout-below-the-range", json!({"sessionTimeoutSeconds": 0})),
        (
            "timeout-above-the-range",
            json!({"sessionTimeoutSeconds": 28_801}),
        ),
        ("certificates-below-its-length", json!({"certificates": []})),
        ("empty-name", json!({"name": ""})),
        ("name-past-its-length", json!({"name": "n".repeat(101)})),
        (
            "client-token-below-its-length",
            json!({"clientToken": "a".repeat(32)}),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = start_session(&router, SYSTEM_INTERPRETER, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            session_count(&engine, SYSTEM_INTERPRETER),
            0,
            "case {case:?}: a refused start must store nothing"
        );
    }
}
