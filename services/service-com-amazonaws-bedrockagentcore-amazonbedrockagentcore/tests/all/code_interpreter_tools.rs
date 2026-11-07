// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore code interpreter TOOL surface:
//! `InvokeCodeInterpreter`, and the session lifecycle it depends on
//! (`StopCodeInterpreterSession` -> `GetCodeInterpreterSession`).
//!
//! The rules under test:
//!
//! * a tool invocation on a live session is served as HTTP 200 whose body is an AWS
//!   event stream carrying ONE `result` event, and whose `content`,
//!   `structuredContent` and `isError` are computed from what the invoked tool did to
//!   that session's own state;
//! * `executeCode` with `language: javascript` RUNS the submitted script — the tables
//!   below assert values no handler could hold as literals (a Fibonacci number, a
//!   sorted array, a closure's result), which is what distinguishes a real engine from
//!   a canned answer — and the scripts a session ran to completion stay in scope for
//!   its later runs unless `clearContext` asks for them to be dropped;
//! * the file tools round-trip the session's workspace, and a path that would leave it
//!   is refused;
//! * a tool this emulator embeds no engine for answers `isError` with the reason and
//!   reports no output, exit status or duration for work nothing performed;
//! * a session that is `TERMINATED` — stopped, or past its time-to-live — serves no
//!   tool, and a stop moves it there (and only the first stop does: a second one
//!   conflicts, while a replayed `clientToken` answers with the stop it performed).
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so what is asserted is the wire behaviour a client sees — status,
//! headers and decoded `vnd.amazon.eventstream` frames — never a handler called
//! directly.

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::date_time::{DateTime, Format};
use aws_smithy_types::event_stream::HeaderValue;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// The system code interpreter AWS ships; `codeInterpreterIdentifier` is modelled as a
/// plain string, so any identifier a caller names addresses its own session set.
const SYSTEM_INTERPRETER: &str = "aws.codeinterpreter.v1";

/// The media type an event-stream body carries.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The header `sessionId` binds on both the request and the response (source:
/// smithy_query query="operation" operation="InvokeCodeInterpreter").
const SESSION_HEADER: &str = "x-amzn-code-interpreter-session-id";

/// The emulator, with request authentication off so a test signs nothing, plus the
/// shared engine so a case can age a stored session's clock without sleeping.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One HTTP exchange.
struct Call {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Call {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, or the
    /// body's `__type`.
    fn error_code(&self) -> String {
        if let Some(value) = self.headers.get("x-amzn-errortype") {
            return value.to_str().unwrap_or_default().to_string();
        }
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    fn media_type(&self) -> String {
        self.header("content-type")
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
    }
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Call {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(value.to_string())
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .dispatch(request.body(payload).expect("build the request"))
        .await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Percent-encode one `@httpLabel` / `@httpQuery` value, the way an AWS SDK does.
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

/// `StartCodeInterpreterSession`, answering with the minted `sessionId`.
async fn start_session(router: &MultiServiceRouter, body: Value) -> String {
    let call = send(
        router,
        "PUT",
        &format!(
            "/code-interpreters/{}/sessions/start",
            label(SYSTEM_INTERPRETER)
        ),
        &[],
        Some(body),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "StartCodeInterpreterSession must answer 200, got {} with {:?}",
        call.status,
        call.json()
    );
    call.json()
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("a started session carries a sessionId")
        .to_string()
}

/// `GetCodeInterpreterSession`.
async fn get_session(router: &MultiServiceRouter, session_id: &str) -> Call {
    send(
        router,
        "GET",
        &format!(
            "/code-interpreters/{}/sessions/get?sessionId={}",
            label(SYSTEM_INTERPRETER),
            label(session_id)
        ),
        &[],
        None,
    )
    .await
}

/// `StopCodeInterpreterSession`.
async fn stop_session(
    router: &MultiServiceRouter,
    session_id: &str,
    client_token: Option<&str>,
) -> Call {
    let mut body = json!({});
    if let Some(token) = client_token {
        body["clientToken"] = json!(token);
    }
    send(
        router,
        "PUT",
        &format!(
            "/code-interpreters/{}/sessions/stop?sessionId={}",
            label(SYSTEM_INTERPRETER),
            label(session_id)
        ),
        &[],
        Some(body),
    )
    .await
}

/// `InvokeCodeInterpreter`: `POST /code-interpreters/{id}/tools/invoke`, with the
/// session in the header the model binds it to.
async fn invoke(
    router: &MultiServiceRouter,
    session_id: Option<&str>,
    name: &str,
    arguments: Value,
) -> Call {
    let mut body = json!({"name": name});
    if !arguments.is_null() {
        body["arguments"] = arguments;
    }
    let headers: Vec<(&str, &str)> = match session_id {
        Some(session) => vec![(SESSION_HEADER, session)],
        None => Vec::new(),
    };
    send(
        router,
        "POST",
        &format!(
            "/code-interpreters/{}/tools/invoke",
            label(SYSTEM_INTERPRETER)
        ),
        &headers,
        Some(body),
    )
    .await
}

/// Decode an event-stream body, asserting the frame headers an SDK decoder requires.
/// Each entry is `(event type, payload)`.
fn decode_frames(case: &str, body: &[u8]) -> Vec<(String, Value)> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            header(":message-type").as_deref(),
            Some("event"),
            "case {case:?}: a response frame must be an event, got {:?}",
            header(":message-type")
        );
        let event_type = header(":event-type")
            .unwrap_or_else(|| panic!("case {case:?}: a response frame named no :event-type"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case:?}: the {event_type} payload must be JSON: {error}")
        });
        frames.push((event_type, payload));
    }
    frames
}

/// The one `result` event a served tool invocation answers with, after asserting the
/// whole wire shape around it.
fn result_event(case: &str, call: &Call) -> Value {
    assert_eq!(
        call.status,
        200,
        "case {case:?}: expected HTTP 200 from InvokeCodeInterpreter, got {} with {:?}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    assert_eq!(
        call.media_type(),
        EVENT_STREAM,
        "case {case:?}: expected an event-stream body, got {:?}",
        call.header("content-type")
    );
    let frames = decode_frames(case, &call.body);
    assert_eq!(
        frames.len(),
        1,
        "case {case:?}: expected exactly one event, got {frames:?}"
    );
    let (event_type, payload) = frames.into_iter().next().expect("one frame");
    assert_eq!(
        event_type, "result",
        "case {case:?}: expected the union's `result` member, got {event_type:?}"
    );
    assert!(
        payload.get("content").and_then(Value::as_array).is_some(),
        "case {case:?}: `content` is required on a CodeInterpreterResult, got {payload}"
    );
    payload
}

fn text_of(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn structured<'a>(result: &'a Value, member: &str) -> &'a Value {
    result
        .get("structuredContent")
        .and_then(|structured| structured.get(member))
        .unwrap_or(&Value::Null)
}

fn is_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Move a stored session's creation instant `by` further into the past, so a case can
/// reach the far side of a time-to-live without sleeping for it. This is the test
/// reaching into the store the handlers share, not an emulator behaviour.
fn age_session(engine: &engine::InMemoryEngine, session_id: &str, by: Duration) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let session = store
        .code_interpreters
        .get_mut(SYSTEM_INTERPRETER)
        .and_then(|held| held.sessions.get_mut(session_id))
        .expect("the session that was just started");
    session.created_at -= by;
}

/// How many files the stored session's workspace holds.
fn workspace_files(engine: &engine::InMemoryEngine, session_id: &str) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store
        .code_interpreter_session(SYSTEM_INTERPRETER, session_id)
        .map_or(0, |session| session.files.len())
}

/// `executeCode` arguments for a JavaScript script.
fn javascript(code: &str) -> Value {
    json!({"code": code, "language": "javascript"})
}

/// `executeCode` with `language: javascript` RUNS the script: each row's expected
/// `stdout` is what the JavaScript language produces for it, computed by the engine
/// rather than held anywhere in this emulator.
///
/// Rows differ along what the rule generalises over: the kind of computation
/// (arithmetic, string, array, object, closure, recursion), the size of the work, the
/// script's own form, which stream it writes to and how it ends.
#[tokio::test]
async fn execute_code_runs_the_submitted_javascript() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // (case, script, expected stdout, expected stderr fragment, expected exit code)
    let cases: Vec<(&str, &str, &str, &str, i64)> = vec![
        ("arithmetic", "console.log(6 * 7)", "42\n", "", 0),
        (
            "string-methods",
            "console.log('héllo wörld'.toUpperCase())",
            "HÉLLO WÖRLD\n",
            "",
            0,
        ),
        (
            "array-sort-and-join",
            "const a = [10, 1, 5, 2]; a.sort((l, r) => l - r); console.log(a.join(','))",
            "1,2,5,10\n",
            "",
            0,
        ),
        (
            "json-round-trip",
            "const v = JSON.parse('{\"b\":[1,2],\"a\":true}'); console.log(JSON.stringify(v))",
            "{\"b\":[1,2],\"a\":true}\n",
            "",
            0,
        ),
        (
            "loop-of-ten-thousand-steps",
            "let s = 0; for (let i = 1; i <= 10000; i++) { s += i; } console.log(s)",
            "50005000\n",
            "",
            0,
        ),
        (
            "recursion",
            "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } console.log(fib(24))",
            "46368\n",
            "",
            0,
        ),
        (
            "closure-and-higher-order-function",
            "const add = x => y => x + y; console.log([1, 2, 3].map(add(10)).join(' '))",
            "11 12 13\n",
            "",
            0,
        ),
        (
            "several-arguments-and-values",
            "console.log('n', 1, true, null, {a: 1}, [1, 2])",
            "n 1 true null {\"a\":1} [1,2]\n",
            "",
            0,
        ),
        (
            "writes-to-the-error-stream",
            "console.log('out'); console.error('problem')",
            "out\n",
            "problem",
            0,
        ),
        (
            "script-that-prints-nothing",
            "const unused = 1 + 1;",
            "",
            "",
            0,
        ),
        (
            "uncaught-exception",
            "console.log('before'); throw new Error('boom')",
            "before\n",
            "boom",
            1,
        ),
        (
            "reference-to-an-undefined-name",
            "console.log(definitelyNotDefined)",
            "",
            "definitelyNotDefined",
            1,
        ),
        (
            // A script the engine cannot compile: the suite asserts that the run failed
            // and said something, not the engine's own wording.
            "syntax-the-engine-cannot-compile",
            "function (",
            "",
            "",
            1,
        ),
    ];

    for (case, script, expected_stdout, expected_stderr, expected_exit) in cases {
        let call = invoke(&router, Some(&session), "executeCode", javascript(script)).await;
        let result = result_event(case, &call);
        assert_eq!(
            call.header(SESSION_HEADER),
            session,
            "case {case:?}: expected the response to name the session the tool ran in"
        );
        assert_eq!(
            structured(&result, "stdout").as_str().unwrap_or_default(),
            expected_stdout,
            "case {case:?}: expected the script's own standard output, got {result}"
        );
        let stderr = structured(&result, "stderr")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            stderr.contains(expected_stderr),
            "case {case:?}: expected the error stream to carry {expected_stderr:?}, got {stderr:?}"
        );
        assert!(
            expected_exit == 0 || !stderr.is_empty(),
            "case {case:?}: a run that failed must say why on its error stream, got {result}"
        );
        assert_eq!(
            structured(&result, "exitCode").as_i64(),
            Some(expected_exit),
            "case {case:?}: expected exit code {expected_exit}, got {result}"
        );
        assert_eq!(
            is_error(&result),
            expected_exit != 0,
            "case {case:?}: isError must follow how the run ended, got {result}"
        );
        assert!(
            structured(&result, "executionTime").as_f64().is_some(),
            "case {case:?}: a run that happened reports how long it took, got {result}"
        );
        // The text content is what a client reads: the run's own output.
        let text = text_of(&result);
        assert!(
            text.contains(expected_stdout.trim()) || text.contains(expected_stderr),
            "case {case:?}: expected the result text to carry the run's output, got {text:?}"
        );
    }
}

/// A script that will not finish is stopped by the engine's own bound, and reported as
/// an error rather than holding the request for ever.
#[tokio::test]
async fn a_script_that_does_not_finish_is_stopped() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    for (case, script) in [
        ("empty-loop", "while (true) {}"),
        ("loop-that-allocates", "const a = []; while (true) { a.push(1); }"),
        (
            "recursion-with-no-base-case",
            "function f(n) { return f(n + 1); } f(0)",
        ),
    ] {
        let call = invoke(&router, Some(&session), "executeCode", javascript(script)).await;
        let result = result_event(case, &call);
        assert!(
            is_error(&result),
            "case {case:?}: a script the engine had to stop is an error, got {result}"
        );
        assert_eq!(
            structured(&result, "exitCode").as_i64(),
            Some(1),
            "case {case:?}: expected a failed run, got {result}"
        );
        assert!(
            !structured(&result, "stderr")
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: expected the reason the run ended, got {result}"
        );
    }

    // The session is still usable afterwards: one run's bound is not the session's end.
    let call = invoke(
        &router,
        Some(&session),
        "executeCode",
        javascript("console.log('still here')"),
    )
    .await;
    let result = result_event("after-a-stopped-script", &call);
    assert_eq!(
        structured(&result, "stdout").as_str(),
        Some("still here\n"),
        "a session serves the next run after one was stopped, got {result}"
    );
}

/// The scripts a session ran to completion stay in scope for its later runs, and
/// `clearContext` drops them. Rows differ along what the earlier run defined.
#[tokio::test]
async fn an_execution_context_carries_across_calls_until_it_is_cleared() {
    let (router, _engine) = emulator();

    // (case, the script that defines something, the script that uses it, expected)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("a-value", "var answer = 42;", "console.log(answer + 1)", "43\n"),
        (
            "a-function",
            "function twice(n) { return n * 2; }",
            "console.log(twice(21))",
            "42\n",
        ),
        (
            "mutated-state",
            "var seen = []; seen.push('first');",
            "seen.push('second'); console.log(seen.join('|'))",
            "first|second\n",
        ),
    ];

    for (case, defines, uses, expected) in cases {
        let session = start_session(&router, json!({})).await;
        let first = invoke(&router, Some(&session), "executeCode", javascript(defines)).await;
        let first = result_event(case, &first);
        assert!(
            !is_error(&first),
            "case {case:?}: the defining run must succeed, got {first}"
        );

        let call = invoke(&router, Some(&session), "executeCode", javascript(uses)).await;
        let result = result_event(case, &call);
        assert_eq!(
            structured(&result, "stdout").as_str(),
            Some(expected),
            "case {case:?}: expected the later run to see what the earlier one defined, got {result}"
        );

        // The same script with the context cleared no longer sees it.
        let cleared = invoke(
            &router,
            Some(&session),
            "executeCode",
            json!({"code": uses, "language": "javascript", "clearContext": true}),
        )
        .await;
        let cleared = result_event(case, &cleared);
        assert!(
            is_error(&cleared),
            "case {case:?}: with the context cleared the later run must not see it, got {cleared}"
        );

        // And the clear is durable: the next run starts from nothing too.
        let after = invoke(&router, Some(&session), "executeCode", javascript(uses)).await;
        let after = result_event(case, &after);
        assert!(
            is_error(&after),
            "case {case:?}: a cleared context stays cleared, got {after}"
        );
    }

    // A run that ended on an exception defines nothing a later run can rely on.
    let session = start_session(&router, json!({})).await;
    let failed = invoke(
        &router,
        Some(&session),
        "executeCode",
        javascript("var half = 1; throw new Error('stop')"),
    )
    .await;
    assert!(
        is_error(&result_event("failed-run", &failed)),
        "the defining run was expected to fail"
    );
    let after = invoke(
        &router,
        Some(&session),
        "executeCode",
        javascript("console.log(typeof half)"),
    )
    .await;
    let after = result_event("after-a-failed-run", &after);
    assert_eq!(
        structured(&after, "stdout").as_str(),
        Some("undefined\n"),
        "a script that threw is not part of the session's context, got {after}"
    );
}

/// The file tools round-trip the session's workspace: what `writeFiles` put there is
/// what `listFiles` enumerates and `readFiles` returns, and `removeFiles` takes it
/// away. Rows differ along the content's form and size and the path's depth.
#[tokio::test]
async fn the_file_tools_round_trip_the_session_workspace() {
    let (router, engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // (case, path, the content block, the bytes it stores)
    let cases: Vec<(&str, &str, Value, Vec<u8>)> = vec![
        (
            "text-at-the-workspace-root",
            "notes.txt",
            json!({"path": "notes.txt", "text": "line one\nline two\n"}),
            b"line one\nline two\n".to_vec(),
        ),
        (
            "text-in-a-nested-directory",
            "data/inner/report.csv",
            json!({"path": "data/inner/report.csv", "text": "a,b\n1,2\n"}),
            b"a,b\n1,2\n".to_vec(),
        ),
        (
            "a-path-written-with-a-leading-dot-segment",
            "data/same.txt",
            json!({"path": "./data/same.txt", "text": "normalized"}),
            b"normalized".to_vec(),
        ),
        (
            "an-empty-file",
            "empty.txt",
            json!({"path": "empty.txt", "text": ""}),
            Vec::new(),
        ),
        (
            "non-ascii-text",
            "unicode.txt",
            json!({"path": "unicode.txt", "text": "αβγ — ok"}),
            "αβγ — ok".as_bytes().to_vec(),
        ),
        (
            "bytes-that-are-not-text",
            "image.bin",
            json!({"path": "image.bin", "blob": aws_smithy_types::base64::encode([0xff_u8, 0x00, 0xfe, 0x80])}),
            vec![0xff, 0x00, 0xfe, 0x80],
        ),
        (
            "a-larger-text-file",
            "big.txt",
            json!({"path": "big.txt", "text": "x".repeat(100_000)}),
            "x".repeat(100_000).into_bytes(),
        ),
    ];

    for (case, key, block, bytes) in &cases {
        let written = invoke(
            &router,
            Some(&session),
            "writeFiles",
            json!({"content": [block]}),
        )
        .await;
        let written = result_event(case, &written);
        assert!(
            !is_error(&written),
            "case {case:?}: expected the write to succeed, got {written}"
        );
        // A workspace tool reports no run, so it carries neither the structured report
        // of one nor an error flag.
        assert!(
            written.get("structuredContent").is_none() && written.get("isError").is_none(),
            "case {case:?}: a workspace tool that did what was asked reports `content` only, \
             got {written}"
        );
        assert!(
            text_of(&written).contains(key),
            "case {case:?}: expected the result to name what was written, got {}",
            text_of(&written)
        );

        let read = invoke(
            &router,
            Some(&session),
            "readFiles",
            json!({"paths": [key]}),
        )
        .await;
        let read = result_event(case, &read);
        assert!(
            !is_error(&read),
            "case {case:?}: expected the read to succeed, got {read}"
        );
        let resource = read
            .get("content")
            .and_then(Value::as_array)
            .and_then(|blocks| blocks.first())
            .and_then(|block| block.get("resource"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            resource.get("uri").and_then(Value::as_str),
            Some(format!("file:///{key}").as_str()),
            "case {case:?}: expected the workspace URI of the file, got {resource}"
        );
        match String::from_utf8(bytes.clone()) {
            Ok(text) => {
                assert_eq!(
                    resource.get("type").and_then(Value::as_str),
                    Some("text"),
                    "case {case:?}: text bytes are reported as text, got {resource}"
                );
                assert_eq!(
                    resource.get("text").and_then(Value::as_str),
                    Some(text.as_str()),
                    "case {case:?}: expected the bytes that were written back"
                );
            }
            Err(_) => {
                assert_eq!(
                    resource.get("type").and_then(Value::as_str),
                    Some("blob"),
                    "case {case:?}: bytes that are not text are reported as a blob, got {resource}"
                );
                assert_eq!(
                    resource.get("blob").and_then(Value::as_str),
                    Some(aws_smithy_types::base64::encode(bytes).as_str()),
                    "case {case:?}: expected the bytes that were written back"
                );
            }
        }
    }

    // Everything written is in the workspace, and `listFiles` enumerates the entries
    // directly under the directory it is asked about.
    assert_eq!(
        workspace_files(&engine, &session),
        cases.len(),
        "every written file must be stored under the session"
    );
    let listed = invoke(&router, Some(&session), "listFiles", json!({})).await;
    let listed = result_event("list-the-workspace-root", &listed);
    let names: Vec<String> = listed
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("uri").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![
            "file:///big.txt",
            "file:///data",
            "file:///empty.txt",
            "file:///image.bin",
            "file:///notes.txt",
            "file:///unicode.txt",
        ],
        "expected the entries directly under the workspace root, got {listed}"
    );
    let nested = invoke(
        &router,
        Some(&session),
        "listFiles",
        json!({"directoryPath": "data"}),
    )
    .await;
    let nested = result_event("list-a-nested-directory", &nested);
    let nested_uris: Vec<String> = nested
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("uri").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        nested_uris,
        vec!["file:///data/inner", "file:///data/same.txt"],
        "expected the entries under `data`, got {nested}"
    );
    let empty = invoke(
        &router,
        Some(&session),
        "listFiles",
        json!({"directoryPath": "nothing/here"}),
    )
    .await;
    let empty = result_event("list-a-directory-with-nothing-in-it", &empty);
    assert_eq!(
        empty.get("content").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "a directory holding nothing lists nothing, got {empty}"
    );

    // `removeFiles` takes several paths at once, and what it removed is gone.
    let removed = invoke(
        &router,
        Some(&session),
        "removeFiles",
        json!({"paths": ["notes.txt", "data/inner/report.csv"]}),
    )
    .await;
    let removed = result_event("remove-two-files", &removed);
    assert!(
        !is_error(&removed),
        "expected the removal to succeed, got {removed}"
    );
    assert_eq!(
        workspace_files(&engine, &session),
        cases.len() - 2,
        "the removed files must be gone from the store"
    );
    let gone = invoke(
        &router,
        Some(&session),
        "readFiles",
        json!({"paths": ["notes.txt"]}),
    )
    .await;
    let gone = result_event("read-a-removed-file", &gone);
    assert!(
        is_error(&gone),
        "a file that is no longer in the workspace is an error, got {gone}"
    );
    assert!(
        gone.get("structuredContent").is_none(),
        "a workspace tool reports no run, even when it failed, got {gone}"
    );
    assert!(
        text_of(&gone).contains("notes.txt"),
        "the error must name the path that was not found, got {}",
        text_of(&gone)
    );
}

/// A tool path addresses the session's workspace and nothing else: a path that is
/// absolute, that walks out of the workspace, or that is past the bounds a workspace
/// path has, is refused with a 400 `ValidationException` and stores nothing.
#[tokio::test]
async fn a_tool_path_that_leaves_the_workspace_is_refused() {
    let (router, engine) = emulator();
    let session = start_session(&router, json!({})).await;

    let long_path = format!("{}.txt", "p".repeat(1_100));
    let deep_path = (0..40).map(|_| "d").collect::<Vec<_>>().join("/");
    let cases: Vec<(&str, String)> = vec![
        ("absolute", "/etc/passwd".to_string()),
        ("parent-segment", "../escape.txt".to_string()),
        ("parent-segment-inside", "data/../../escape.txt".to_string()),
        ("only-separators", "///".to_string()),
        ("past-the-path-length", long_path),
        ("past-the-segment-count", deep_path),
    ];

    for (case, path) in cases {
        let write = invoke(
            &router,
            Some(&session),
            "writeFiles",
            json!({"content": [{"path": path, "text": "x"}]}),
        )
        .await;
        assert_eq!(
            write.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            write.status,
            write.json()
        );
        assert_eq!(
            write.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            write.json()
        );

        let read = invoke(
            &router,
            Some(&session),
            "readFiles",
            json!({"paths": [path]}),
        )
        .await;
        assert_eq!(
            read.status, 400,
            "case {case:?}: a read of the same path is refused the same way, got {}",
            read.status
        );
    }
    assert_eq!(
        workspace_files(&engine, &session),
        0,
        "a refused path must store nothing"
    );
}

/// A tool call addresses at most as many files as one call may, and a repeated path
/// addresses the same file once: neither the work nor the response grows with how many
/// times a caller names something.
#[tokio::test]
async fn a_tool_call_cannot_name_more_files_than_one_call_carries() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // `StringList` and `InputContentBlockList` carry no @length (source: smithy_query
    // query="shape" shape="StringList,InputContentBlockList"), so the ceiling is the
    // emulator's own: 64 files per call.
    let within: Vec<String> = (0..64).map(|index| format!("f{index}.txt")).collect();
    let past: Vec<String> = (0..65).map(|index| format!("f{index}.txt")).collect();

    let blocks: Vec<Value> = within
        .iter()
        .map(|path| json!({"path": path, "text": "x"}))
        .collect();
    let written = invoke(
        &router,
        Some(&session),
        "writeFiles",
        json!({"content": blocks}),
    )
    .await;
    assert!(
        !is_error(&result_event("the-most-files-one-call-carries", &written)),
        "64 files in one call is within the ceiling"
    );

    for (case, tool, arguments) in [
        (
            "write-past-the-ceiling",
            "writeFiles",
            json!({"content": past.iter().map(|path| json!({"path": path, "text": "x"})).collect::<Vec<_>>()}),
        ),
        ("read-past-the-ceiling", "readFiles", json!({"paths": past})),
        (
            "remove-past-the-ceiling",
            "removeFiles",
            json!({"paths": past}),
        ),
    ] {
        let call = invoke(&router, Some(&session), tool, arguments).await;
        assert_eq!(
            call.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            call.status,
            call.json()
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.json()
        );
    }

    // A path named several times is read once.
    let read = invoke(
        &router,
        Some(&session),
        "readFiles",
        json!({"paths": ["f0.txt", "f0.txt", "./f0.txt", "f1.txt"]}),
    )
    .await;
    let read = result_event("a-repeated-path", &read);
    assert_eq!(
        read.get("content").and_then(Value::as_array).map(Vec::len),
        Some(2),
        "a repeated path addresses the same file once, got {read}"
    );
}

/// A read whose result would be larger than one result carries is refused rather than
/// materialised: the response is bounded by the call, not by the count of files times
/// the size of each.
#[tokio::test]
async fn a_read_result_larger_than_one_result_carries_is_refused() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // Four files of 1.5 MB each: any one of them is readable, and all four together are
    // past the 4 MB one result carries.
    let paths: Vec<String> = (0..4).map(|index| format!("big{index}.txt")).collect();
    for path in &paths {
        let written = invoke(
            &router,
            Some(&session),
            "writeFiles",
            json!({"content": [{"path": path, "text": "y".repeat(1_500_000)}]}),
        )
        .await;
        assert!(
            !is_error(&result_event("write-a-large-file", &written)),
            "the workspace must take a 1.5 MB file"
        );
    }

    let one = invoke(
        &router,
        Some(&session),
        "readFiles",
        json!({"paths": [paths.first()]}),
    )
    .await;
    assert!(
        !is_error(&result_event("read-one-large-file", &one)),
        "one 1.5 MB file is within what a result carries"
    );

    let all = invoke(&router, Some(&session), "readFiles", json!({"paths": paths})).await;
    let all = result_event("read-four-large-files", &all);
    assert!(
        is_error(&all),
        "a result past the bytes one read carries is refused, got {all}"
    );
    assert!(
        text_of(&all).contains("readFiles"),
        "the refusal must say which tool could not answer, got {}",
        text_of(&all)
    );
}

/// A `writeFiles` call the workspace has no room for stores NOTHING: the batch is
/// weighed before any of its files lands, so a refused call leaves the workspace
/// exactly as it was.
#[tokio::test]
async fn a_write_the_workspace_has_no_room_for_stores_nothing() {
    let (router, engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // The workspace holds at most 8 MiB across every file in it, so six 1.5 MB files in
    // one call is past it while any one of them fits.
    let blocks: Vec<Value> = (0..6)
        .map(|index| json!({"path": format!("b{index}.txt"), "text": "z".repeat(1_500_000)}))
        .collect();
    let refused = invoke(
        &router,
        Some(&session),
        "writeFiles",
        json!({"content": blocks}),
    )
    .await;
    let refused = result_event("a-batch-past-the-workspace-ceiling", &refused);
    assert!(
        is_error(&refused),
        "a batch the workspace cannot hold is an error, got {refused}"
    );
    assert_eq!(
        workspace_files(&engine, &session),
        0,
        "a refused batch must leave the workspace as it was"
    );

    // The same files written one call at a time land until the ceiling is reached, and
    // the call that meets it stores nothing either.
    let mut stored = 0;
    for index in 0..6 {
        let call = invoke(
            &router,
            Some(&session),
            "writeFiles",
            json!({"content": [{"path": format!("b{index}.txt"), "text": "z".repeat(1_500_000)}]}),
        )
        .await;
        let result = result_event("one-file-at-a-time", &call);
        if !is_error(&result) {
            stored += 1;
        }
        assert_eq!(
            workspace_files(&engine, &session),
            stored,
            "case {index}: the workspace holds exactly the files that were accepted"
        );
    }
    assert!(
        (1..6).contains(&stored),
        "some files must have been accepted and the ceiling must have stopped the rest, got \
         {stored}"
    );
}

/// The tools plane is bounded ACROSS the whole store, not only per session: once every
/// session together holds `MAX_STORED_SESSION_BYTES`, a further workspace write is
/// refused and stores nothing, and it is accepted again as soon as the store has room.
///
/// The per-session ceilings alone would admit their product — one full workspace per
/// session, on every code interpreter — so the rule under test is the whole-store one.
/// The other sessions are seeded directly into the store (the CONTROL plane does not
/// create them, and driving 64 MiB through the wire would only slow the case down);
/// everything the rule is asserted on goes through real HTTP.
#[tokio::test]
async fn the_whole_store_bounds_every_session_workspace_together() {
    use agentcore::state::{
        CodeInterpreterSessionRecord, SessionFile, MAX_SESSION_FILE_BYTES,
        MAX_STORED_SESSION_BYTES,
    };

    let (router, engine) = emulator();
    let session = start_session(&router, json!({})).await;

    /// Seed one session holding a full workspace on its own code interpreter, and answer
    /// that interpreter's identifier so the case can drop it again.
    fn seed_full_session(engine: &engine::InMemoryEngine, index: usize) -> String {
        let identifier = format!("filler_interpreter-{index:010}");
        let state = agentcore::state::service_state(engine).expect("the service state");
        let mut store = state.lock();
        let seq = store.next_seq();
        let mut seeded = CodeInterpreterSessionRecord::started(
            seq,
            identifier.clone(),
            None,
            900,
            None,
            None,
            std::time::SystemTime::now(),
        );
        seeded.files.insert(
            "filler.bin".to_string(),
            SessionFile {
                bytes: vec![0u8; MAX_SESSION_FILE_BYTES],
                written_at: std::time::SystemTime::now(),
            },
        );
        store.insert_code_interpreter_session(seeded, None);
        identifier
    }

    // Fill the store to its ceiling with full workspaces of other sessions.
    let fillers: Vec<String> = (0..MAX_STORED_SESSION_BYTES / MAX_SESSION_FILE_BYTES)
        .map(|index| seed_full_session(&engine, index))
        .collect();

    // A one-byte write into the live session now has nowhere to go, and the refusal says
    // it is the store that is full rather than this session's own workspace.
    let refused = invoke(
        &router,
        Some(&session),
        "writeFiles",
        json!({"content": [{"path": "small.txt", "text": "x"}]}),
    )
    .await;
    let refused = result_event("the-store-is-full", &refused);
    assert!(
        is_error(&refused),
        "case the-store-is-full: a write past the whole-store ceiling is an error, got {refused}"
    );
    assert!(
        text_of(&refused).contains(&MAX_STORED_SESSION_BYTES.to_string()),
        "case the-store-is-full: the refusal must name the whole-store ceiling, got {:?}",
        text_of(&refused)
    );
    assert_eq!(
        workspace_files(&engine, &session),
        0,
        "case the-store-is-full: a refused write must store nothing, got {} file(s)",
        workspace_files(&engine, &session)
    );

    // Drop one of the full sessions: the store has room again, and the very same write is
    // accepted — so the ceiling is weighed against what is HELD, not a blanket refusal.
    {
        let state = agentcore::state::service_state(&*engine).expect("the service state");
        let mut store = state.lock();
        store
            .code_interpreters
            .remove(fillers.first().expect("one filler interpreter"));
    }
    let accepted = invoke(
        &router,
        Some(&session),
        "writeFiles",
        json!({"content": [{"path": "small.txt", "text": "x"}]}),
    )
    .await;
    let accepted = result_event("the-store-has-room-again", &accepted);
    assert!(
        !is_error(&accepted),
        "case the-store-has-room-again: expected the write to be accepted, got {accepted}"
    );
    assert_eq!(
        workspace_files(&engine, &session),
        1,
        "case the-store-has-room-again: expected the file stored, got {} file(s)",
        workspace_files(&engine, &session)
    );
}

/// `executeCode` runs the script when the runtime named is one the embedded engine
/// serves, and reports that nothing ran for any other.
#[tokio::test]
async fn only_a_runtime_the_engine_serves_runs_the_script() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // `LanguageRuntime` is nodejs | deno | python (source: smithy_query query="shape"
    // shape="LanguageRuntime"); the first two run JavaScript, the third does not.
    for (case, runtime, runs) in [
        ("no-runtime-named", None, true),
        ("nodejs", Some("nodejs"), true),
        ("deno", Some("deno"), true),
        ("python", Some("python"), false),
    ] {
        let mut arguments = javascript("console.log('ok')");
        if let Some(runtime) = runtime {
            arguments["runtime"] = json!(runtime);
        }
        let call = invoke(&router, Some(&session), "executeCode", arguments).await;
        let result = result_event(case, &call);
        if runs {
            assert_eq!(
                structured(&result, "stdout").as_str(),
                Some("ok\n"),
                "case {case:?}: expected the script to run, got {result}"
            );
        } else {
            assert!(
                is_error(&result) && structured(&result, "stdout").is_null(),
                "case {case:?}: expected nothing to have run, got {result}"
            );
        }
    }
}

/// A tool invocation that leaves out an argument the tool cannot work without is a 400
/// `ValidationException`, never a 200 with an invented answer.
#[tokio::test]
async fn a_tool_invocation_missing_a_required_argument_is_refused() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        ("execute-code-with-no-code", "executeCode", json!({"language": "javascript"})),
        ("execute-code-with-no-language", "executeCode", json!({"code": "1"})),
        ("execute-code-with-no-arguments", "executeCode", Value::Null),
        ("read-files-with-no-paths", "readFiles", json!({})),
        ("remove-files-with-no-paths", "removeFiles", json!({})),
        ("write-files-with-no-content", "writeFiles", json!({})),
        ("write-files-with-an-empty-content-list", "writeFiles", json!({"content": []})),
    ];

    for (case, tool, arguments) in cases {
        let call = invoke(&router, Some(&session), tool, arguments).await;
        assert_eq!(
            call.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            call.status,
            call.json()
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.json()
        );
    }
}

/// A tool this emulator embeds no engine for says so: `isError`, the reason in the
/// result, and NO output, exit status or duration for work nothing performed.
#[tokio::test]
async fn a_tool_with_no_engine_reports_that_nothing_ran() {
    let (router, _engine) = emulator();
    let session = start_session(&router, json!({})).await;

    // Rows: the `ProgrammingLanguage` values with no engine here, the `LanguageRuntime`
    // one, and every tool of the command plane (source: smithy_query query="shape"
    // shape="ProgrammingLanguage,LanguageRuntime,ToolName").
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "python-code",
            "executeCode",
            json!({"code": "print(1)", "language": "python"}),
        ),
        (
            "typescript-code",
            "executeCode",
            json!({"code": "const a: number = 1;", "language": "typescript"}),
        ),
        (
            "javascript-on-the-python-runtime",
            "executeCode",
            json!({"code": "console.log(1)", "language": "javascript", "runtime": "python"}),
        ),
        (
            "a-command",
            "executeCommand",
            json!({"command": "ls -la"}),
        ),
        (
            "a-command-started-as-a-task",
            "startCommandExecution",
            json!({"command": "sleep 1"}),
        ),
        ("a-task-read-back", "getTask", json!({"taskId": "task-1"})),
        ("a-task-stopped", "stopTask", json!({"taskId": "task-1"})),
    ];

    for (case, tool, arguments) in cases {
        let call = invoke(&router, Some(&session), tool, arguments).await;
        let result = result_event(case, &call);
        assert!(
            is_error(&result),
            "case {case:?}: work nothing ran is an error, got {result}"
        );
        assert!(
            !text_of(&result).is_empty(),
            "case {case:?}: expected the result to say what was not run, got {result}"
        );
        assert!(
            structured(&result, "exitCode").is_null(),
            "case {case:?}: nothing ran, so there is no exit status, got {result}"
        );
        assert!(
            structured(&result, "executionTime").is_null(),
            "case {case:?}: nothing ran, so there is no duration, got {result}"
        );
        assert!(
            structured(&result, "stdout").is_null(),
            "case {case:?}: nothing ran, so there is no output, got {result}"
        );
        assert!(
            !structured(&result, "stderr")
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: expected the reason on the error stream, got {result}"
        );
    }
}

/// A tool call that names no session is served in a session the call starts, which the
/// response names back and a later read reports as a session of that code interpreter.
#[tokio::test]
async fn a_tool_call_that_names_no_session_is_served_in_one_it_starts() {
    let (router, _engine) = emulator();

    let call = invoke(
        &router,
        None,
        "executeCode",
        javascript("console.log(2 ** 10)"),
    )
    .await;
    let result = result_event("no-session-named", &call);
    assert_eq!(
        structured(&result, "stdout").as_str(),
        Some("1024\n"),
        "the tool must still have run, got {result}"
    );
    let session = call.header(SESSION_HEADER);
    assert!(
        (1..=40).contains(&session.len()) && session.bytes().all(|b| b.is_ascii_alphanumeric()),
        "expected a minted sessionId matching ^[0-9a-zA-Z]{{1,40}}$, got {session:?}"
    );
    let read = get_session(&router, &session).await;
    assert_eq!(
        (read.status, read.json().get("status").and_then(Value::as_str)),
        (200, Some("READY")),
        "the session the call started must be readable and live, got {:?}",
        read.json()
    );

    // A second call naming that session runs in it, context and all.
    let again = invoke(
        &router,
        Some(&session),
        "executeCode",
        javascript("console.log('same session')"),
    )
    .await;
    let again = result_event("the-session-it-started", &again);
    assert_eq!(
        structured(&again, "stdout").as_str(),
        Some("same session\n"),
        "got {again}"
    );
}

/// A session this emulator does not hold serves no tool, and neither does one that is
/// `TERMINATED` — stopped, or past the time-to-live it was started with.
#[tokio::test]
async fn a_session_that_is_not_live_serves_no_tool() {
    let (router, engine) = emulator();

    // Unknown session on a code interpreter that holds one.
    let live = start_session(&router, json!({})).await;
    let unknown = invoke(
        &router,
        Some("0123456789abcdef"),
        "executeCode",
        javascript("console.log(1)"),
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.error_code().as_str()),
        (404, "ResourceNotFoundException"),
        "an unknown session is a 404, got {:?}",
        unknown.json()
    );
    assert!(
        !live.is_empty(),
        "the live session is what makes the unknown one a per-session answer"
    );

    // A stopped session.
    let stopped = start_session(&router, json!({})).await;
    let stop = stop_session(&router, &stopped, None).await;
    assert_eq!(stop.status, 200, "expected the stop to be served");
    for (case, tool, arguments) in [
        ("execute-code", "executeCode", javascript("console.log(1)")),
        ("list-files", "listFiles", json!({})),
    ] {
        let call = invoke(&router, Some(&stopped), tool, arguments).await;
        assert_eq!(
            (call.status, call.error_code().as_str()),
            (409, "ConflictException"),
            "case {case:?}: a stopped session cannot run a tool, got {:?}",
            call.json()
        );
    }

    // A session whose time-to-live has run out, with no stop at all.
    let expired = start_session(&router, json!({"sessionTimeoutSeconds": 60})).await;
    age_session(&engine, &expired, Duration::from_secs(60));
    let call = invoke(
        &router,
        Some(&expired),
        "executeCode",
        javascript("console.log(1)"),
    )
    .await;
    assert_eq!(
        (call.status, call.error_code().as_str()),
        (409, "ConflictException"),
        "a session past its time-to-live cannot run a tool, got {:?}",
        call.json()
    );
}

/// `StopCodeInterpreterSession` moves the session to `TERMINATED`, which is what every
/// later read reports; a second stop conflicts with that state, while a replayed
/// `clientToken` answers with the stop it already performed.
#[tokio::test]
async fn a_stop_terminates_the_session_for_every_later_read() {
    let (router, engine) = emulator();

    // Rows differ along the time-to-live the session was started with (both published
    // bounds of `CodeInterpreterSessionTimeout` 1..28800 and its default) and whether
    // the stop carried a `clientToken`: the transition is the same in each.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("default-timeout-no-token", json!({}), None),
        (
            "longest-timeout-no-token",
            json!({"sessionTimeoutSeconds": 28_800}),
            None,
        ),
        (
            "named-session-with-a-token",
            json!({"name": "teardown", "sessionTimeoutSeconds": 120}),
            Some("stop-token-0123456789-abcdefghij-0"),
        ),
    ];

    for (case, start, token) in cases {
        let session = start_session(&router, start).await;
        let before = get_session(&router, &session).await;
        assert_eq!(
            before.json().get("status").and_then(Value::as_str),
            Some("READY"),
            "case {case:?}: a started session is READY, got {:?}",
            before.json()
        );
        // Something in the workspace, so the stop's effect on it is observable.
        let written = invoke(
            &router,
            Some(&session),
            "writeFiles",
            json!({"content": [{"path": "kept.txt", "text": "x"}]}),
        )
        .await;
        assert!(!is_error(&result_event(case, &written)));

        let stop = stop_session(&router, &session, token).await;
        assert_eq!(
            stop.status, 200,
            "case {case:?}: expected HTTP 200 from the stop, got {:?}",
            stop.json()
        );
        let body = stop.json();
        assert_eq!(
            body.get("sessionId").and_then(Value::as_str),
            Some(session.as_str()),
            "case {case:?}: expected the stopped session back, got {body}"
        );
        assert_eq!(
            body.get("codeInterpreterIdentifier").and_then(Value::as_str),
            Some(SYSTEM_INTERPRETER),
            "case {case:?}: expected the code interpreter the session was started on, got {body}"
        );
        let last_updated = body
            .get("lastUpdatedAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            DateTime::from_str(&last_updated, Format::DateTime).is_ok(),
            "case {case:?}: `lastUpdatedAt` targets DateTimestamp, so it is an RFC 3339 string, \
             got {body}"
        );

        let after = get_session(&router, &session).await;
        assert_eq!(
            (
                after.status,
                after.json().get("status").and_then(Value::as_str)
            ),
            (200, Some("TERMINATED")),
            "case {case:?}: a stopped session reads back as TERMINATED, got {:?}",
            after.json()
        );
        // A read reports it without changing it.
        let again = get_session(&router, &session).await;
        assert_eq!(
            again.json(),
            after.json(),
            "case {case:?}: a second read reports the same session"
        );
        assert_eq!(
            workspace_files(&engine, &session),
            0,
            "case {case:?}: a stop releases what the session held"
        );

        // A second stop. With the same token it is the stop that already happened;
        // without one it conflicts with a session that is no longer running.
        let second = stop_session(&router, &session, token).await;
        match token {
            Some(_) => {
                assert_eq!(
                    second.status, 200,
                    "case {case:?}: a replayed clientToken is not refused, got {:?}",
                    second.json()
                );
                assert_eq!(
                    second.json().get("lastUpdatedAt").and_then(Value::as_str),
                    Some(last_updated.as_str()),
                    "case {case:?}: a replay answers with the stop it already performed, got {:?}",
                    second.json()
                );
            }
            None => {
                assert_eq!(
                    (second.status, second.error_code().as_str()),
                    (409, "ConflictException"),
                    "case {case:?}: stopping a terminated session conflicts, got {:?}",
                    second.json()
                );
            }
        }

        // A fresh token on an already-terminated session is a conflict too: the token
        // only speaks for the request it was first sent with.
        let fresh = stop_session(
            &router,
            &session,
            Some("another-token-0123456789-abcdefghij"),
        )
        .await;
        assert_eq!(
            (fresh.status, fresh.error_code().as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected a conflict for a new token on a stopped session, got {:?}",
            fresh.json()
        );
    }

    // A session this emulator does not hold cannot be stopped.
    let unknown = stop_session(&router, "0123456789abcdef", None).await;
    assert_eq!(
        (unknown.status, unknown.error_code().as_str()),
        (404, "ResourceNotFoundException"),
        "stopping an unknown session is a 404, got {:?}",
        unknown.json()
    );

    // A session whose time-to-live has run out is already terminated, so a stop
    // conflicts with it as well.
    let expired = start_session(&router, json!({"sessionTimeoutSeconds": 1})).await;
    age_session(&engine, &expired, Duration::from_secs(1));
    let stop = stop_session(&router, &expired, None).await;
    assert_eq!(
        (stop.status, stop.error_code().as_str()),
        (409, "ConflictException"),
        "stopping a session past its time-to-live conflicts, got {:?}",
        stop.json()
    );
}
