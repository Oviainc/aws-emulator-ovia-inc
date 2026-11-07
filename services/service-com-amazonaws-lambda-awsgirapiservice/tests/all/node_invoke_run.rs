// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for invoking a function whose `Runtime` is a NODE one: `Invoke` with
//! `InvocationType=RequestResponse` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`).
//!
//! The rule under test: a synchronous invocation of a `Zip` function whose `Runtime` names
//! a language this emulator embeds an engine for RUNS the handler out of the stored
//! deployment package and answers what the run produced. For a node function that means
//! the package's own module is loaded — as CommonJS or as an ES module, as Node decides
//! which a file is — the handler is called with the event and the context, and its value is
//! the response payload; an error it raised is still an HTTP 200 carrying the
//! `errorType`/`errorMessage`/`trace` document the node runtime reports and the
//! `X-Amz-Function-Error: Unhandled` signal; `LogType=Tail` carries the log the run wrote;
//! and the function's `Timeout` and `MemorySize` end a run that exceeds either.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what
//! the rule generalises over: which module system and handler style the package is written
//! in, what the handler computes, how it ends, which `Runtime` identifier the function
//! carries, what it imports, and which bound it runs past.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(100);

/// The `Runtime` every row that does not vary it uses.
const RUNTIME: &str = "nodejs20.x";

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
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
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

/// A .zip deployment package holding `entries`, written in memory the way a caller uploads
/// one.
///
/// `pub(crate)` so a sibling suite that packages a node handler drives the same writer
/// rather than a third copy of it.
pub(crate) fn package(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, source) in entries {
            writer.start_file(*name, options).expect("the entry starts");
            writer
                .write_all(source.as_bytes())
                .expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// What a function this suite creates is configured with.
struct Function<'a> {
    name: &'a str,
    entries: &'a [(&'a str, &'a str)],
    handler: &'a str,
    runtime: &'a str,
    timeout: i64,
    memory_size: i64,
    variables: Value,
}

impl<'a> Function<'a> {
    /// A function with this suite's defaults, holding `entries`.
    fn of(name: &'a str, entries: &'a [(&'a str, &'a str)]) -> Self {
        Self {
            name,
            entries,
            handler: "index.handler",
            runtime: RUNTIME,
            timeout: 5,
            memory_size: 256,
            variables: json!({}),
        }
    }
}

/// Create `function` and wait out the provisioning window the create documents.
async fn create_function(router: &MultiServiceRouter, function: &Function<'_>) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": function.name,
            "Role": ROLE,
            "Runtime": function.runtime,
            "Handler": function.handler,
            "Timeout": function.timeout,
            "MemorySize": function.memory_size,
            "Environment": { "Variables": function.variables },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(function.entries)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {:?} must succeed, got {} {}",
        function.name,
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Delete the function `name`, so the next row of a table has the name free again.
async fn delete_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let deleted = call(router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[], &[]).await;
    assert_eq!(
        deleted.status, 204,
        "case {case}: the function must be removable between rows, got {}",
        deleted.text()
    );
}

/// `Invoke` on the function `reference` names.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    payload: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/invocations"),
        payload,
        headers,
    )
    .await
}

/// The entries of one deployment package: the archive-relative path of each file and its
/// source.
type Entries<'a> = &'a [(&'a str, &'a str)];

/// One row of the payload table: the case, the package, the `Handler` and the JSON the
/// invocation is expected to answer with.
type PayloadCase<'a> = (&'a str, Entries<'a>, &'a str, Value);

/// One row of the failure table: the case, the package, the `Handler`, and the error type
/// and message the invocation is expected to report.
type FailureCase<'a> = (&'a str, Entries<'a>, &'a str, &'a str, &'a str);

/// One row of the bound table: the case, the handler's source, the function's `Timeout`
/// and `MemorySize`, and the message and error type the run is expected to end with.
type BoundCase<'a> = (&'a str, &'a str, i64, i64, &'a str, Option<&'a str>);

/// The handler's own value is the response payload — one row per module system, handler
/// style and kind of value a node handler computes, so the answer is the RUN's and not a
/// shape this emulator fills in.
#[tokio::test]
async fn the_response_payload_is_what_a_node_handler_returned() {
    let cases: &[PayloadCase<'_>] = &[
        (
            "a CommonJS async handler",
            &[(
                "index.js",
                "exports.handler = async (event) => ({ ok: true, echo: event });",
            )],
            "index.handler",
            json!({"ok": true, "echo": {"a": 1}}),
        ),
        (
            "a CommonJS handler returning a promise",
            &[(
                "index.js",
                "module.exports.handler = function (event) { \
                 return Promise.resolve({ doubled: event.a * 2 }); };",
            )],
            "index.handler",
            json!({"doubled": 2}),
        ),
        (
            "a CommonJS callback handler",
            &[(
                "index.js",
                "exports.handler = function (event, context, callback) { \
                 callback(null, { called: true }); };",
            )],
            "index.handler",
            json!({"called": true}),
        ),
        (
            "a callback handler answering from a timer",
            &[(
                "index.js",
                "exports.handler = function (event, context, callback) { \
                 setTimeout(function () { callback(null, { late: true }); }, 10); };",
            )],
            "index.handler",
            json!({"late": true}),
        ),
        (
            "an ES module handler",
            &[(
                "index.mjs",
                "export const handler = async (event) => ({ esm: true, keys: Object.keys(event) });",
            )],
            "index.handler",
            json!({"esm": true, "keys": ["a"]}),
        ),
        (
            "an ES module handler of a package declared a module",
            &[
                ("package.json", r#"{"type":"module"}"#),
                (
                    "index.js",
                    "export const handler = async () => ({ declared: 'module' });",
                ),
            ],
            "index.handler",
            json!({"declared": "module"}),
        ),
        (
            "a handler in a nested module of the package",
            &[
                (
                    "src/app.js",
                    "const helper = require('./helper');\n\
                     exports.run = async (event) => ({ helped: helper.twice(event.a) });\n",
                ),
                ("src/helper.js", "exports.twice = (n) => n * 2;\n"),
            ],
            "src/app.run",
            json!({"helped": 2}),
        ),
        (
            "a handler reading the environment and the context",
            &[(
                "index.js",
                "exports.handler = async (event, context) => ({\n\
                     secret: process.env.SECRET,\n\
                     name: context.functionName,\n\
                     version: context.functionVersion,\n\
                     memory: context.memoryLimitInMB,\n\
                     remaining: context.getRemainingTimeInMillis() > 0,\n\
                 });\n",
            )],
            "index.handler",
            json!({
                "secret": "shhh",
                "name": "probe-node",
                "version": "$LATEST",
                "memory": "256",
                "remaining": true,
            }),
        ),
        (
            // The node runtime IGNORES what a NON-async handler returns: only an async
            // handler's promise or the callback answers an invocation, so this one is
            // answered when the event loop empties, with no value at all.
            "a non-async handler whose return value the runtime ignores",
            &[(
                "index.js",
                "exports.handler = function (event) { return { ignored: event.a }; };",
            )],
            "index.handler",
            Value::Null,
        ),
        (
            "a handler using a core module this emulator supplies",
            &[(
                "index.js",
                "const path = require('node:path');\n\
                 const util = require('util');\n\
                 const EventEmitter = require('events');\n\
                 exports.handler = async () => {\n\
                     const bus = new EventEmitter();\n\
                     let seen = null;\n\
                     bus.on('ping', (value) => { seen = value; });\n\
                     bus.emit('ping', 'pong');\n\
                     return {\n\
                         joined: path.join('/var/task', 'lib', '..', 'index.js'),\n\
                         formatted: util.format('%s:%d', 'n', 2),\n\
                         seen: seen,\n\
                     };\n\
                 };\n",
            )],
            "index.handler",
            json!({
                "joined": "/var/task/index.js",
                "formatted": "n:2",
                "seen": "pong",
            }),
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, entries, handler, expected) in cases {
        let name = "probe-node";
        let mut function = Function::of(name, entries);
        function.handler = handler;
        function.variables = json!({"SECRET": "shhh"});
        create_function(&router, &function).await;
        let answered = invoke(&router, name, br#"{"a":1}"#, &[]).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.json(),
            *expected,
            "case {case}: expected {expected}, got {}",
            answered.text()
        );
        // The payload is a DOCUMENT, never an empty body: an invocation the runtime has no
        // value for answers the literal `null`, which `Answer::json` reads the same way an
        // empty body would, so the body itself is checked here.
        assert!(
            !answered.text().is_empty(),
            "case {case}: expected a payload document, got an empty body"
        );
        assert_eq!(
            answered.header("x-amz-function-error"),
            None,
            "case {case}: a handler that returned reports no function error"
        );
        assert_eq!(
            answered.header("x-amz-executed-version").as_deref(),
            Some("$LATEST"),
            "case {case}: expected the executed version, got {:?}",
            answered.header("x-amz-executed-version")
        );
        delete_function(&router, case, name).await;
    }
}

/// Every `Runtime` identifier of the node language runs the same package — one row per
/// identifier class of the model's `Runtime` enum (`smithy_query(query="shape",
/// shape="Runtime")`), including the two the report named and ones it did not.
#[tokio::test]
async fn every_node_runtime_runs_its_handler() {
    let runtimes = ["nodejs18.x", "nodejs20.x", "nodejs22.x", "nodejs24.x"];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for runtime in runtimes {
        let name = "probe-runtime";
        let entries: &[(&str, &str)] = &[(
            "index.js",
            "exports.handler = async (event) => ({ ok: true, echo: event });",
        )];
        let mut function = Function::of(name, entries);
        function.runtime = runtime;
        create_function(&router, &function).await;
        let answered = invoke(&router, name, br#"{"a":1}"#, &[]).await;
        assert_eq!(
            answered.status,
            200,
            "case {runtime}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.json(),
            json!({"ok": true, "echo": {"a": 1}}),
            "case {runtime}: expected the handler's own value, got {}",
            answered.text()
        );
        delete_function(&router, runtime, name).await;
    }
}

/// An error the handler raised is still an HTTP 200, carrying the document the node runtime
/// reports it with — `errorType`, `errorMessage` and `trace` — and the `Unhandled` signal.
/// One row per way a node handler fails.
#[tokio::test]
async fn an_unhandled_node_handler_error_is_a_two_hundred_with_the_trace_document() {
    let cases: &[FailureCase<'_>] = &[
        (
            "a thrown Error",
            &[(
                "index.js",
                "exports.handler = async () => { throw new Error('boom'); };",
            )],
            "index.handler",
            "Error",
            "boom",
        ),
        (
            "a failed computation",
            &[(
                "index.js",
                "exports.handler = async () => { return undefined.member; };",
            )],
            "index.handler",
            "TypeError",
            "undefined",
        ),
        (
            "a rejected promise",
            &[(
                "index.js",
                "exports.handler = () => Promise.reject(new RangeError('out of range'));",
            )],
            "index.handler",
            "RangeError",
            "out of range",
        ),
        (
            "an error passed to the callback",
            &[(
                "index.js",
                "exports.handler = function (event, context, callback) { \
                 callback(new Error('callback failed')); };",
            )],
            "index.handler",
            "Error",
            "callback failed",
        ),
        (
            "a handler the module does not export",
            &[("index.js", "exports.other = async () => 1;")],
            "index.handler",
            "Runtime.HandlerNotFound",
            "index.handler is undefined or not a function",
        ),
        (
            "a module that does not load",
            &[("index.js", "require('./absent');")],
            "index.handler",
            "Runtime.ImportModuleError",
            "Cannot find module './absent'",
        ),
        (
            "source that does not compile",
            &[("index.js", "exports.handler = async () => { this is not javascript };")],
            "index.handler",
            "Runtime.UserCodeSyntaxError",
            "",
        ),
        (
            "a module the package does not hold",
            &[("index.js", "exports.handler = async () => 1;")],
            "absent.handler",
            "Runtime.ImportModuleError",
            "Cannot find module 'absent'",
        ),
        (
            "an error thrown from inside a timer",
            &[(
                "index.js",
                "exports.handler = function (event, context, callback) { \
                 setTimeout(function () { throw new Error('late failure'); }, 10); };",
            )],
            "index.handler",
            "Error",
            "late failure",
        ),
        (
            "a value the response cannot be serialized from",
            &[(
                "index.js",
                "exports.handler = async () => { const held = {}; held.self = held; \
                 return held; };",
            )],
            "index.handler",
            "TypeError",
            "circular",
        ),
        (
            "a malformed handler name",
            &[("index.js", "exports.handler = async () => 1;")],
            "index",
            "Runtime.MalformedHandlerName",
            "Bad handler 'index'",
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, entries, handler, error_type, message) in cases {
        let name = "probe-node-error";
        let mut function = Function::of(name, entries);
        function.handler = handler;
        create_function(&router, &function).await;
        let answered = invoke(&router, name, b"{}", &[]).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: an unhandled error is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {case}: expected the Unhandled signal, got {:?}",
            answered.header("x-amz-function-error")
        );
        let document = answered.json();
        assert_eq!(
            document.get("errorType").and_then(Value::as_str),
            Some(*error_type),
            "case {case}: expected the error type {error_type}, got {document}"
        );
        let reported = document
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reported.contains(message),
            "case {case}: expected {message:?} in the message, got {document}"
        );
        // The node runtime reports a handler's stack under `trace`, which is the member
        // this document carries it under.
        assert!(
            document.get("trace").is_some_and(Value::is_array),
            "case {case}: expected the trace array, got {document}"
        );
        assert!(
            document.get("stackTrace").is_none(),
            "case {case}: stackTrace is another runtime's member, got {document}"
        );
        delete_function(&router, case, name).await;
    }
}

/// The log tail carries what the run wrote to the console, and only when `LogType=Tail`
/// asked for it.
#[tokio::test]
async fn the_log_tail_carries_what_a_node_handler_wrote() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let entries: &[(&str, &str)] = &[(
        "index.js",
        "exports.handler = async () => { \
         console.log('handler says', { n: 1 }); \
         console.error('and warns'); \
         return { ok: true }; };",
    )];
    create_function(&router, &Function::of("probe-node-log", entries)).await;
    let cases: &[(&str, Option<&str>, bool)] = &[
        ("no LogType", None, false),
        ("LogType=None", Some("None"), false),
        ("LogType=Tail", Some("Tail"), true),
    ];
    for (case, log_type, reported) in cases {
        let headers: Vec<(&str, &str)> = log_type
            .map(|value| vec![("x-amz-log-type", value)])
            .unwrap_or_default();
        let answered = invoke(&router, "probe-node-log", b"{}", &headers).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        let tail = answered.header("x-amz-log-result");
        assert_eq!(
            tail.is_some(),
            *reported,
            "case {case}: expected a log tail {reported}, got {tail:?}"
        );
        if let Some(tail) = tail {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(tail)
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                .unwrap_or_else(|error| panic!("case {case}: the tail is base64: {error}"));
            for expected in [
                "START RequestId:",
                "handler says { n: 1 }",
                "and warns",
                "END RequestId:",
                "REPORT",
            ] {
                assert!(
                    decoded.contains(expected),
                    "case {case}: expected {expected:?} in the log, got {decoded:?}"
                );
            }
        }
    }
}

/// A run that exceeds the function's `Timeout` or its `MemorySize` ends with the error
/// Lambda reports for that case — one row per bound.
#[tokio::test]
async fn a_run_past_the_functions_own_bounds_ends_with_lambdas_error() {
    let cases: &[BoundCase<'_>] = &[
        (
            "a handler that never returns",
            "exports.handler = async () => { for (;;) {} };",
            1,
            256,
            "Task timed out after 1.00 seconds",
            None,
        ),
        (
            "a handler that never stops waiting",
            "exports.handler = function (event, context, callback) { \
             setTimeout(function () { callback(null, { late: true }); }, 600000); };",
            1,
            256,
            "Task timed out after 1.00 seconds",
            None,
        ),
        (
            "a handler that allocates past its memory",
            "exports.handler = async () => { const held = []; \
             for (;;) { held.push(new Array(100000).fill(7)); } };",
            10,
            128,
            "Runtime exited with error: signal: killed",
            Some("Runtime.ExitError"),
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, source, timeout, memory_size, message, error_type) in cases {
        let name = "probe-node-bound";
        let entries: &[(&str, &str)] = &[("index.js", source)];
        let mut function = Function::of(name, entries);
        function.timeout = *timeout;
        function.memory_size = *memory_size;
        create_function(&router, &function).await;
        let answered = invoke(&router, name, b"{}", &[]).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: a run its bounds ended is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {case}: expected the Unhandled signal, got {:?}",
            answered.header("x-amz-function-error")
        );
        let document = answered.json();
        let reported = document
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            reported.contains(message),
            "case {case}: expected {message:?} in the message, got {document}"
        );
        if let Some(error_type) = error_type {
            assert_eq!(
                document.get("errorType").and_then(Value::as_str),
                Some(*error_type),
                "case {case}: expected the error type {error_type}, got {document}"
            );
        }
        delete_function(&router, case, name).await;
    }
}

/// Handler code reaches nothing of the HOST: one row per thing a node program could
/// otherwise reach a host file, process or library with, each of which resolves to nothing
/// here — so the run fails the way it would with the dependency missing.
#[tokio::test]
async fn a_node_handler_cannot_reach_the_host() {
    let cases: &[(&str, &str)] = &[
        (
            "a host file, read through the filesystem",
            "require('fs').readFileSync('/etc/passwd')",
        ),
        (
            "a write outside the one writable directory",
            "require('fs').writeFileSync('/var/task/written', 'x')",
        ),
        ("a child process", "require('child_process')"),
        ("a socket", "require('net')"),
        ("an http client", "require('http')"),
        ("a native binding", "process.binding('fs')"),
        ("a host path", "require('/etc/passwd')"),
        ("a path above the task root", "require('../../../etc/passwd')"),
        (
            "a traversal under the task root",
            "require('/var/task/../../../etc/passwd')",
        ),
        ("the module loader itself", "require('module')"),
        ("a dynamic host import", "import('/etc/passwd')"),
        ("a dynamic traversal import", "import('../../../etc/passwd')"),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, reach) in cases {
        let name = "probe-node-host";
        let source =
            format!("exports.handler = async () => {{ await ({reach}); return {{ reached: true }}; }};");
        let entries: &[(&str, &str)] = &[("index.js", source.as_str())];
        create_function(&router, &Function::of(name, entries)).await;
        let answered = invoke(&router, name, b"{}", &[]).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {case}: reaching the host must fail the run, got {}",
            answered.text()
        );
        assert_eq!(
            answered.json().get("reached"),
            None,
            "case {case}: nothing of the host is reachable, got {}",
            answered.text()
        );
        delete_function(&router, case, name).await;
    }
}

/// The environment a run sees is the function's own: no host environment variable and no
/// credential of this emulator is in it.
#[tokio::test]
async fn a_node_handler_sees_only_the_functions_own_environment() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let entries: &[(&str, &str)] = &[(
        "index.js",
        "exports.handler = async () => {\n\
             const absent = ['PATH', 'HOME', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY']\n\
                 .filter((name) => process.env[name] !== undefined);\n\
             return { secret: process.env.SECRET, absent: absent, cwd: process.cwd() };\n\
         };\n",
    )];
    let name = "probe-node-env";
    let mut function = Function::of(name, entries);
    function.variables = json!({"SECRET": "shhh"});
    create_function(&router, &function).await;
    let answered = invoke(&router, name, b"{}", &[]).await;
    assert_eq!(
        answered.status,
        200,
        "expected 200, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.json(),
        json!({"secret": "shhh", "absent": [], "cwd": "/var/task"}),
        "expected the function's own environment and nothing of the host, got {}",
        answered.text()
    );
}
