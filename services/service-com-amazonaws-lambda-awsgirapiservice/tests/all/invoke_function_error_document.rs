// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DOCUMENT a failed invocation is answered with: `Invoke` with
//! `InvocationType=RequestResponse` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`), whose response payload is that
//! document when `X-Amz-Function-Error` is set.
//!
//! Three rules are under test, each over a table of inputs:
//!
//! * a run the SANDBOX ended because it outlasted the function's `Timeout` carries the
//!   sandbox timeout class as its `errorType`, beside a message whose fixed text states
//!   the timeout the run was given;
//! * every frame of the stack such a document carries is ONE traceback frame — its
//!   location line and, where the source is known, the source line at it — whose file
//!   field is the path of a source file, with no fine-grained error-location anchors
//!   under it;
//! * the request id the document names is the invocation's only for a failure the
//!   invocation was bound to: a failure raised while the runtime was still resolving the
//!   handler happens before any invocation is bound, and names none.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what
//! the rule generalises over: the `Timeout` the run is given and the runtime that runs it,
//! how deep the stack of the failure is and whether it reaches the runtime's own modules,
//! and which phase of the run the failure was raised in.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::{is_host_bound, is_request_id, one_invocation_stream_at_a_time};
// The one multi-entry deployment-archive writer this target has, which a package of any
// language's modules is written by.
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The header AWS reports a response's request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The `Runtime` every Python row runs.
const PYTHON: &str = "python3.12";

/// The `Runtime` the node row runs.
const NODE: &str = "nodejs20.x";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The id this answer names its request by.
    fn request_id(&self) -> String {
        self.header(REQUEST_ID_HEADER).unwrap_or_default()
    }

    /// The member `name` of the answered document, as text.
    fn member(&self, name: &str) -> Option<String> {
        self.json()
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The frames of the stack the document carries, under the member the runtime of
    /// `runtime` reports it as.
    fn frames(&self, runtime: &str) -> Vec<String> {
        let member = if runtime.starts_with("nodejs") {
            "trace"
        } else {
            "stackTrace"
        };
        self.json()
            .get(member)
            .and_then(Value::as_array)
            .map(|frames| {
                frames
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request, with the headers `headers` names.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Create a function holding `entries`, configured with `handler`, `runtime` and
/// `timeout`, and wait out the provisioning window the create documents.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    entries: &[(&str, &str)],
    handler: &str,
    runtime: &str,
    timeout: i64,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": handler,
            "Timeout": timeout,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(entries)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Remove the function `name`, so the next row of a table creates its own.
async fn delete_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let deleted = call(router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[], &[]).await;
    assert_eq!(
        deleted.status, 204,
        "case {case}: the function must be removable between rows, got {} {}",
        deleted.status,
        deleted.text()
    );
}

/// `Invoke` on the function `reference` names, re-driven while the answer reports one of the
/// emulator's own HOST BOUNDS rather than what the run came to ([`host_bound`]): every suite
/// of this test binary contends for the emulator's run slots, whose refusal says to retry the
/// request, and for the resident size it guards the whole process with, which stops whichever
/// run is in flight. An answer of the run's own — whatever it reports — is returned at once.
async fn invoke(router: &MultiServiceRouter, reference: &str, payload: &[u8]) -> Answer {
    let mut answered = invoke_once(router, reference, payload).await;
    for _ in 1..SLOT_ATTEMPTS {
        if !host_bound(&answered) {
            break;
        }
        tokio::time::sleep(SLACK).await;
        answered = invoke_once(router, reference, payload).await;
    }
    answered
}

/// How many times a call waits out a host bound before its answer is read as it is.
const SLOT_ATTEMPTS: usize = 6;

/// One `Invoke` on the function `reference` names.
async fn invoke_once(router: &MultiServiceRouter, reference: &str, payload: &[u8]) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/invocations"),
        payload,
        &[],
    )
    .await
}

/// Whether `answered` reports one of the EMULATOR's own host bounds instead of what the
/// run came to — read by the one reading of it this target has
/// ([`crate::invoke_request_id::is_host_bound`]).
///
/// A row answered with one is not read as the run's: the resident-size ceiling the
/// emulator guards this whole process with ends whichever run is in flight once the test
/// binary as a whole is over it, which says nothing about the document under test. The
/// same three rules are pinned without a run at all — over the document builder and the
/// Python runtime's own frames — by this crate's own tests in
/// `src/state/execution.rs` and `src/state/execution/python.rs`.
fn host_bound(answered: &Answer) -> bool {
    is_host_bound(answered.status, &answered.text())
}

/// The lines of one rendered traceback frame.
fn frame_lines(frame: &str) -> Vec<&str> {
    frame
        .strip_suffix('\n')
        .unwrap_or(frame)
        .split('\n')
        .collect()
}

/// The file, line number and name a frame's LOCATION line states, or `None` when the line
/// is not one.
fn located(line: &str) -> Option<(String, u32, String)> {
    let rest = line.strip_prefix("  File \"")?;
    let (file, rest) = rest.split_once("\", line ")?;
    let (number, name) = rest.split_once(", in ")?;
    Some((file.to_string(), number.parse().ok()?, name.to_string()))
}

/// Whether `line` is a fine-grained error-location ANCHOR — the run of `~` and `^` an
/// interpreter can render under a source line to point at part of it.
fn is_anchor(line: &str) -> bool {
    let marks = line.trim();
    !marks.is_empty() && marks.chars().all(|mark| matches!(mark, '~' | '^'))
}

/// Assert that every frame of `frames` is one frame as a runtime renders it: a location
/// line naming a source FILE by path, at the line of a real source line and in a named
/// function, at most one source line under it, and no anchors.
fn assert_frames_are_locations_and_source(case: &str, frames: &[String]) {
    for (index, frame) in frames.iter().enumerate() {
        assert!(
            frame.ends_with('\n'),
            "case {case}: frame {index} must end its last line, got {frame:?}"
        );
        let lines = frame_lines(frame);
        assert!(
            lines.len() <= 2,
            "case {case}: frame {index} is a location and at most its source line, got \
             {lines:?}"
        );
        let Some((file, number, name)) = located(lines[0]) else {
            panic!("case {case}: frame {index} must open with its location, got {frame:?}");
        };
        assert!(
            file.starts_with('/') && file.ends_with(".py"),
            "case {case}: frame {index} must name a source file by path, got {file:?}"
        );
        assert!(
            number > 0 && !name.is_empty(),
            "case {case}: frame {index} must state the line it is at and the function it is \
             in, got line {number} in {name:?}"
        );
        for (line_number, line) in lines.iter().enumerate().skip(1) {
            assert!(
                !is_anchor(line),
                "case {case}: frame {index} line {line_number} is an error-location anchor, \
                 which no frame carries, got {line:?}"
            );
            assert!(
                line.starts_with("    "),
                "case {case}: frame {index} line {line_number} must be the indented source \
                 line, got {line:?}"
            );
        }
    }
}

/// One row of the timeout table: the runtime the function runs, the module to package,
/// the handler it names and the `Timeout` the run is given.
struct TimeoutRow<'a> {
    case: &'a str,
    runtime: &'a str,
    entry: (&'a str, &'a str),
    handler: &'a str,
    timeout: i64,
}

/// A run the sandbox ended because it outlasted the function's own `Timeout` reports the
/// sandbox timeout class, and a message stating the timeout it was given — one row per
/// `Timeout` and per runtime, so neither the class nor the message is one run's.
#[tokio::test]
async fn a_timed_out_run_reports_the_sandbox_timeout_class() {
    let _stream = one_invocation_stream_at_a_time().await;
    let cases: &[TimeoutRow<'_>] = &[
        TimeoutRow {
            case: "a python handler that sleeps past a one-second timeout",
            runtime: PYTHON,
            entry: (
                "lambda_function.py",
                "import time\n\
                 def lambda_handler(event, context):\n    time.sleep(30)\n",
            ),
            handler: "lambda_function.lambda_handler",
            timeout: 1,
        },
        TimeoutRow {
            case: "a python handler that never returns, past a two-second timeout",
            runtime: PYTHON,
            entry: (
                "lambda_function.py",
                "def lambda_handler(event, context):\n    \
                 while True:\n        pass\n",
            ),
            handler: "lambda_function.lambda_handler",
            timeout: 2,
        },
        TimeoutRow {
            case: "a node handler that never returns, past a one-second timeout",
            runtime: NODE,
            entry: ("index.js", "exports.handler = async () => { for (;;) {} };"),
            handler: "index.handler",
            timeout: 1,
        },
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut read = 0_usize;
    for row in cases {
        let case = row.case;
        let name = "probe-timeout-document";
        create_function(
            &router,
            name,
            &[row.entry],
            row.handler,
            row.runtime,
            row.timeout,
        )
        .await;
        let answered = invoke(&router, name, b"{}").await;
        if host_bound(&answered) {
            delete_function(&router, case, name).await;
            continue;
        }
        read += 1;
        assert_eq!(
            answered.status,
            200,
            "case {case}: a timed-out run is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {case}: expected the Unhandled signal, got {:?}",
            answered.header("x-amz-function-error")
        );
        assert_eq!(
            answered.member("errorType").as_deref(),
            Some("Sandbox.Timedout"),
            "case {case}: expected the sandbox timeout class, got {:?}",
            answered.member("errorType")
        );
        let message = answered.member("errorMessage").unwrap_or_default();
        let stated = format!("Task timed out after {:.2} seconds", row.timeout as f64);
        assert!(
            message.ends_with(&stated),
            "case {case}: expected the message to state {stated:?}, got {message:?}"
        );
        let leading = message[..message.len() - stated.len()].trim();
        assert!(
            is_request_id(leading),
            "case {case}: expected the invocation's id before the message, got {leading:?}"
        );
        delete_function(&router, case, name).await;
    }
    assert!(
        read > 0,
        "no row of this table was answered by a run at all: every one was ended by this \
         process's own host bounds, so the rule was never observed"
    );
}

/// Every frame of the stack a failed run reports is ONE frame as the runtime renders it:
/// its location and the source line at it, with the file named by path — one row per shape
/// of stack, including one that reaches the runtime's own modules, whose frame names the
/// module's source file under the runtime's library directory.
#[tokio::test]
async fn a_reported_frame_is_a_location_and_its_source_line() {
    let _stream = one_invocation_stream_at_a_time().await;
    // The library directory of the runtime the function is configured for, where a managed
    // runtime keeps the standard library it ships.
    let library: &str = &format!("/var/lang/lib/{PYTHON}/");
    // The case, the module source, the event, how many frames the stack holds at least,
    // and the directory a frame of the stack must name a file in beside the task root.
    let cases: &[(&str, &str, &str, usize, Option<&str>)] = &[
        (
            "a subscript of a key the event does not hold",
            "def lambda_handler(event, context):\n    \
             if event[\"status\"] == \"SUCCEEDED\":\n        return {\"ok\": True}\n",
            "{}",
            1,
            None,
        ),
        (
            "a raise two calls deep in the package's own module",
            "def inner(value):\n    raise ValueError(\"no good: %s\" % value)\n\
             def lambda_handler(event, context):\n    return inner(event[\"n\"])\n",
            "{\"n\": 7}",
            2,
            None,
        ),
        (
            "a read of an environment variable the function does not set",
            "import os\n\
             def lambda_handler(event, context):\n    \
             return {\"table\": os.environ[\"ABSENT_TABLE_NAME\"]}\n",
            "{}",
            2,
            Some(library),
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut read = 0_usize;
    for (case, source, event, least, within) in cases {
        let name = "probe-frame-document";
        create_function(
            &router,
            name,
            &[("lambda_function.py", source)],
            "lambda_function.lambda_handler",
            PYTHON,
            5,
        )
        .await;
        let answered = invoke(&router, name, event.as_bytes()).await;
        if host_bound(&answered) {
            delete_function(&router, case, name).await;
            continue;
        }
        read += 1;
        assert_eq!(
            answered.status,
            200,
            "case {case}: a raised exception is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        let frames = answered.frames(PYTHON);
        assert!(
            frames.len() >= *least,
            "case {case}: expected at least {least} frames, got {frames:?}"
        );
        assert_frames_are_locations_and_source(case, &frames);
        let (file, _, _) = located(frame_lines(&frames[0])[0])
            .unwrap_or_else(|| panic!("case {case}: a located first frame, got {frames:?}"));
        assert_eq!(
            file, "/var/task/lambda_function.py",
            "case {case}: the first frame is the handler's own module, got {file:?}"
        );
        if let Some(within) = within {
            assert!(
                frames.iter().any(|frame| {
                    located(frame_lines(frame)[0])
                        .is_some_and(|(file, _, _)| file.starts_with(within))
                }),
                "case {case}: expected a frame naming a source file in {within:?}, got \
                 {frames:?}"
            );
        }
        delete_function(&router, case, name).await;
    }
    assert!(
        read > 0,
        "no row of this table was answered by a run at all: every one was ended by this \
         process's own host bounds, so the rule was never observed"
    );
}

/// One row of the phase table: the module to package, the handler the function is
/// configured with, the error class the run comes to and whether an invocation was bound
/// to the run when it failed.
struct PhaseRow<'a> {
    case: &'a str,
    entry: (&'a str, &'a str),
    handler: &'a str,
    error_type: &'a str,
    bound: bool,
}

/// The request id a failed run's document names is the INVOCATION's only for a failure the
/// invocation was bound to: one raised while the runtime was still resolving the handler
/// names none — one row per phase a failure is raised in.
#[tokio::test]
async fn an_initialization_failure_names_no_invocation() {
    let _stream = one_invocation_stream_at_a_time().await;
    let cases: &[PhaseRow<'_>] = &[
        PhaseRow {
            case: "a handler naming a module the package does not hold",
            entry: ("index.py", "def handler(event, context):\n    return {}\n"),
            handler: "main.handler",
            error_type: "Runtime.ImportModuleError",
            bound: false,
        },
        PhaseRow {
            case: "an entry point the module does not define",
            entry: (
                "lambda_function.py",
                "def other(event, context):\n    return {}\n",
            ),
            handler: "lambda_function.lambda_handler",
            error_type: "Runtime.HandlerNotFound",
            bound: false,
        },
        PhaseRow {
            case: "a handler name with no module to split off",
            entry: (
                "lambda_function.py",
                "def lambda_handler(event, context):\n    return {}\n",
            ),
            handler: "lambda_handler",
            error_type: "Runtime.MalformedHandlerName",
            bound: false,
        },
        PhaseRow {
            case: "an exception the handler raised",
            entry: (
                "lambda_function.py",
                "def lambda_handler(event, context):\n    raise ValueError(\"boom\")\n",
            ),
            handler: "lambda_function.lambda_handler",
            error_type: "ValueError",
            bound: true,
        },
        PhaseRow {
            case: "a value the handler cannot marshal",
            entry: (
                "lambda_function.py",
                "def lambda_handler(event, context):\n    return {\"s\": set()}\n",
            ),
            handler: "lambda_function.lambda_handler",
            error_type: "Runtime.MarshalError",
            bound: true,
        },
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut read = 0_usize;
    for row in cases {
        let case = row.case;
        let error_type = row.error_type;
        let name = "probe-init-document";
        create_function(&router, name, &[row.entry], row.handler, PYTHON, 5).await;
        let answered = invoke(&router, name, b"{}").await;
        if host_bound(&answered) {
            delete_function(&router, case, name).await;
            continue;
        }
        read += 1;
        assert_eq!(
            answered.status,
            200,
            "case {case}: a failed run is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.member("errorType").as_deref(),
            Some(error_type),
            "case {case}: expected the error class {error_type}, got {}",
            answered.text()
        );
        let reported = answered.member("requestId").unwrap_or_else(|| {
            panic!(
                "case {case}: the document names a request id, got {}",
                answered.text()
            )
        });
        if row.bound {
            assert_eq!(
                reported,
                answered.request_id(),
                "case {case}: a failure of the invocation names that invocation, got \
                 {reported:?}"
            );
            assert!(
                is_request_id(&reported),
                "case {case}: expected an invocation id, got {reported:?}"
            );
        } else {
            assert_eq!(
                reported, "",
                "case {case}: a failure raised before an invocation is bound names none, got \
                 {reported:?}"
            );
            assert!(
                is_request_id(&answered.request_id()),
                "case {case}: the ANSWER still names the request it answered, got {:?}",
                answered.request_id()
            );
        }
        delete_function(&router, case, name).await;
    }
    assert!(
        read > 0,
        "no row of this table was answered by a run at all: every one was ended by this \
         process's own host bounds, so the rule was never observed"
    );
}
