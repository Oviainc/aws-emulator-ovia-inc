// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a NODE runtime gives the handler it runs, and for what it
//! reports when that handler fails: `Invoke` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`) on a `nodejs*` function.
//!
//! The rules under test:
//!
//! * a managed node runtime's own LIBRARY is on the handler's resolver path and its GLOBALS
//!   are defined, so a handler that uses the standard library, the bundled AWS SDK, a
//!   package vendored in its own `node_modules` or one shipped in a layer, or a runtime
//!   global, returns a value instead of failing to import — and the layer path is searched
//!   after the function's own, so the function's copy of a package wins;
//! * a node runtime's FUNCTION-ERROR payload is exactly `{errorType, errorMessage, trace}`,
//!   whose first trace line is the `"<ErrorType>: <message>"` line the thrown value makes;
//! * an invocation node cannot complete — module syntax in a CommonJS file, a promise
//!   rejection nothing handled, a runtime option node does not know, a handler that never
//!   settles — is reported as a FAILED invocation rather than as a success.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what
//! each rule generalises over: which module of the library is reached and how it is spelled,
//! where the package being imported lives, which global is read, what kind of value the
//! handler threw, and which way the invocation could not complete.

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

/// Where `PublishLayerVersion` is bound.
const LAYERS_URI: &str = "/2018-10-31/layers";

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
const RUNTIME: &str = "nodejs22.x";

/// The header a failed invocation names the failure in.
const FUNCTION_ERROR_HEADER: &str = "x-amz-function-error";

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

    fn function_error(&self) -> Option<String> {
        self.headers
            .get(FUNCTION_ERROR_HEADER)
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

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
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

/// A .zip archive holding `entries`, written in memory the way a caller uploads one.
fn package(entries: &[(&str, &str)]) -> Vec<u8> {
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

/// The base64 of an archive holding `entries`, as a `ZipFile` member carries it.
fn zip_file(entries: &[(&str, &str)]) -> String {
    base64::engine::general_purpose::STANDARD.encode(package(entries))
}

/// Create a function holding `entries`, and wait out the provisioning window the create
/// documents.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    entries: &[(&str, &str)],
    extra: Value,
) {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": RUNTIME,
        "Handler": "index.handler",
        "Timeout": 5,
        "MemorySize": 256,
        "Code": { "ZipFile": zip_file(entries) },
    });
    let object = body.as_object_mut().expect("the create body is an object");
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        object.insert(member.clone(), value.clone());
    }
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&body).expect("the create body serializes"),
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

/// Publish a layer version holding `entries` and answer with its ARN.
async fn publish_layer(router: &MultiServiceRouter, name: &str, entries: &[(&str, &str)]) -> String {
    let published = call(
        router,
        "POST",
        &format!("{LAYERS_URI}/{name}/versions"),
        &serde_json::to_vec(&json!({
            "Content": { "ZipFile": zip_file(entries) },
            "CompatibleRuntimes": [RUNTIME],
        }))
        .expect("the publish body serializes"),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "PublishLayerVersion {name:?} must succeed, got {} {}",
        published.status,
        published.text()
    );
    published.json()["LayerVersionArn"]
        .as_str()
        .expect("the published version's ARN")
        .to_string()
}

/// `Invoke` the function `name` with `payload`.
async fn invoke(router: &MultiServiceRouter, name: &str, payload: &[u8]) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        payload,
    )
    .await
}

/// `Invoke` the function `name`, asking for the log tail, and answer with the response and
/// the decoded log.
async fn invoke_with_log(router: &MultiServiceRouter, name: &str) -> (Answer, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "http://lambda.us-east-1.amazonaws.com{FUNCTIONS_URI}/{name}/invocations"
        ))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .header("x-amz-log-type", "Tail")
        .body(bytes::Bytes::from_static(b"{}"))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let tail = parts
        .headers
        .get("x-amz-log-result")
        .and_then(|value| value.to_str().ok())
        .map(|encoded| {
            String::from_utf8_lossy(
                &base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .expect("the log tail is base64"),
            )
            .to_string()
        })
        .unwrap_or_default();
    (
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        },
        tail,
    )
}

/// The entries of one archive: each file's archive-relative path and its source.
type Entries<'a> = &'a [(&'a str, &'a str)];

/// One row of the library table: the case, the expression the handler answers with, and the
/// value that expression must produce.
type LibraryCase<'a> = (&'a str, &'a str, Value);

/// A handler that answers with `expression`, as a one-file CommonJS package.
fn answering(expression: &str) -> String {
    format!("exports.handler = async () => ({{ value: {expression} }});")
}

/// The managed runtime's own library and globals are reachable from a handler — one row per
/// module of the standard library a handler reaches for, per spelling of its name, and per
/// runtime global it reads. Every row RETURNS a computed value, which is what proves the
/// module was there rather than merely importable.
#[tokio::test]
async fn a_node_handler_reaches_the_managed_runtimes_library_and_globals() {
    let cases: &[LibraryCase<'_>] = &[
        (
            "path",
            "require('path').join('a', 'b', '../c')",
            json!("a/c"),
        ),
        (
            "node:path, under the prefix a modern import spells it with",
            "require('node:path').basename('/var/task/index.js')",
            json!("index.js"),
        ),
        (
            "crypto, computing a published digest",
            "require('crypto').createHash('sha256').update('abc').digest('hex')",
            json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        ),
        (
            "crypto, drawing random bytes of the length asked for",
            "require('crypto').randomBytes(16).length",
            json!(16),
        ),
        (
            "crypto, refusing a draw past the size one may ask for",
            "(() => { try { require('crypto').randomBytes(1e12); return null; } \
             catch (error) { return error.code; } })()",
            json!("ERR_OUT_OF_RANGE"),
        ),
        (
            "fs, writing and reading back the writable directory",
            "(() => { const fs = require('fs'); fs.writeFileSync('/tmp/kept.txt', 'held'); \
             return fs.readFileSync('/tmp/kept.txt', 'utf8'); })()",
            json!("held"),
        ),
        (
            "fs, refusing a write outside the writable directory",
            "(() => { try { require('fs').writeFileSync('/var/task/x', 'y'); return null; } \
             catch (error) { return error.code; } })()",
            json!("EROFS"),
        ),
        (
            "timers/promises, awaited",
            "await require('timers/promises').setTimeout(5, 'waited')",
            json!("waited"),
        ),
        (
            "os, naming the writable directory",
            "require('os').tmpdir()",
            json!("/tmp"),
        ),
        (
            "util, formatting",
            "require('util').format('%s=%d', 'n', 7)",
            json!("n=7"),
        ),
        (
            "events, dispatching",
            "(() => { const E = require('events'); const e = new E(); let seen = null; \
             e.on('x', (v) => { seen = v; }); e.emit('x', 5); return seen; })()",
            json!(5),
        ),
        (
            "stream, composing a sink",
            "typeof new (require('stream').PassThrough)().write",
            json!("function"),
        ),
        (
            "url, converting a file url to its path",
            "require('url').fileURLToPath('file:///var/task/index.js')",
            json!("/var/task/index.js"),
        ),
        (
            "the bundled AWS SDK for JavaScript v3",
            "typeof require('@aws-sdk/client-s3').S3Client",
            json!("function"),
        ),
        (
            "the bundled SDK's DynamoDB document client",
            "typeof require('@aws-sdk/lib-dynamodb').DynamoDBDocumentClient.from",
            json!("function"),
        ),
        (
            "the bundled SDK's value conversions",
            "require('@aws-sdk/util-dynamodb').marshall({ n: 1 }).n.N",
            json!("1"),
        ),
        (
            "the Buffer global, round-tripping a payload through base64",
            "Buffer.from(Buffer.from('ünïcødé').toString('base64'), 'base64').toString('utf8')",
            json!("ünïcødé"),
        ),
        (
            "the process global, naming the runtime's release line",
            "process.versions.node.split('.')[0]",
            json!("22"),
        ),
        (
            "the process global, naming the architecture the function is configured for",
            "process.arch",
            json!("x64"),
        ),
        (
            "the Intl global, naming the zone the environment sets",
            "Intl.DateTimeFormat().resolvedOptions().timeZone",
            json!("UTC"),
        ),
        (
            "a Date read in the zone the environment sets",
            "new Date(1768448200000).toString()",
            json!("Thu Jan 15 2026 03:36:40 GMT+0000 (UTC)"),
        ),
        (
            "the URL global",
            "new URL('https://host/p?q=1').pathname",
            json!("/p"),
        ),
        (
            "the TextEncoder global",
            "new TextEncoder().encode('ab').length",
            json!(2),
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, expression, expected)) in cases.iter().enumerate() {
        let name = format!("library-{index}");
        let source = answering(expression);
        create_function(
            &router,
            &name,
            &[("index.js", source.as_str())],
            json!({}),
        )
        .await;
        let answered = invoke(&router, &name, b"{}").await;
        assert_eq!(
            answered.function_error(),
            None,
            "case {case}: the invocation must report no function error, got {}",
            answered.text()
        );
        let actual = answered.json();
        assert_eq!(
            actual,
            json!({ "value": expected }),
            "case {case}: expected {expected}, got {actual}"
        );
    }
}

/// A package a handler imports by its bare name is resolved from the function's own
/// `node_modules` first and from the layer's afterwards, so the function's copy SHADOWS the
/// layer's — one row per place the package can live, and one for the layer directory of
/// another major version of the runtime, which is on no search path at all.
#[tokio::test]
async fn a_bare_import_resolves_the_functions_own_packages_before_a_layers() {
    const HANDLER: &str = "exports.handler = async () => ({ \
         value: (() => { try { return require('dep').from; } \
         catch (error) { return error.code; } })() });";
    let own: Entries<'_> = &[
        ("index.js", HANDLER),
        ("node_modules/dep/index.js", "exports.from = 'the package';"),
    ];
    let bare: Entries<'_> = &[("index.js", HANDLER)];
    let layer_current: Entries<'_> = &[(
        "nodejs/node_modules/dep/index.js",
        "exports.from = 'the layer';",
    )];
    let layer_other_major: Entries<'_> = &[(
        "nodejs/node14/node_modules/dep/index.js",
        "exports.from = 'another major version';",
    )];
    let cases: &[(&str, Entries<'_>, Option<Entries<'_>>, Value)] = &[
        ("the function's own node_modules", own, None, json!("the package")),
        (
            "a layer's version-independent directory",
            bare,
            Some(layer_current),
            json!("the layer"),
        ),
        (
            "the function's own copy shadowing the layer's",
            own,
            Some(layer_current),
            json!("the package"),
        ),
        (
            "a layer directory of another major version",
            bare,
            Some(layer_other_major),
            json!("MODULE_NOT_FOUND"),
        ),
        ("no copy anywhere", bare, None, json!("MODULE_NOT_FOUND")),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, entries, layer, expected)) in cases.iter().enumerate() {
        let name = format!("resolution-{index}");
        let extra = match layer {
            None => json!({}),
            Some(entries) => {
                let arn = publish_layer(&router, &format!("layer-{index}"), entries).await;
                json!({ "Layers": [arn] })
            }
        };
        create_function(&router, &name, entries, extra).await;
        let answered = invoke(&router, &name, b"{}").await;
        assert_eq!(
            answered.function_error(),
            None,
            "case {case}: the invocation must report no function error, got {}",
            answered.text()
        );
        let actual = answered.json();
        assert_eq!(
            actual,
            json!({ "value": expected }),
            "case {case}: expected {expected}, got {actual}"
        );
    }
}

/// `TZ` is the function's own to set: the run reads its local clock in the zone the
/// function's environment names, and in UTC when it names none — one row per class of zone
/// (no zone at all, one with a whole-hour offset, one with a half-hour offset, one whose
/// offset depends on the date), read at one fixed instant so the expected reading is the
/// zone database's and not the day's.
#[tokio::test]
async fn the_local_clock_is_the_zone_the_function_configures() {
    // 2026-01-15T03:30:00Z, in northern winter, so a zone with daylight saving is read at
    // its standard offset.
    const INSTANT: i64 = 1_768_448_200_000;
    let cases: &[(&str, Option<&str>, &str, i64, i64)] = &[
        ("no zone configured", None, "UTC", 0, 3),
        ("a whole-hour offset", Some("Asia/Tokyo"), "Asia/Tokyo", -540, 12),
        (
            "a half-hour offset",
            Some("Asia/Kolkata"),
            "Asia/Kolkata",
            -330,
            9,
        ),
        (
            "a zone on its standard offset",
            Some("America/New_York"),
            "America/New_York",
            300,
            22,
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, zone, resolved, offset_minutes, local_hour)) in cases.iter().enumerate() {
        let name = format!("zone-{index}");
        let source = format!(
            "exports.handler = async () => {{ const at = new Date({INSTANT}); return {{ \
             tz: process.env.TZ, offsetMinutes: at.getTimezoneOffset(), \
             localHour: at.getHours(), \
             resolved: Intl.DateTimeFormat().resolvedOptions().timeZone }}; }};"
        );
        let extra = match zone {
            None => json!({}),
            Some(named) => json!({ "Environment": { "Variables": { "TZ": named } } }),
        };
        create_function(&router, &name, &[("index.js", source.as_str())], extra).await;
        let answered = invoke(&router, &name, b"{}").await;
        assert_eq!(
            answered.function_error(),
            None,
            "case {case}: the invocation must report no function error, got {}",
            answered.text()
        );
        let actual = answered.json();
        assert_eq!(
            actual,
            json!({
                "tz": zone.unwrap_or(":UTC"),
                "offsetMinutes": offset_minutes,
                "localHour": local_hour,
                "resolved": resolved,
            }),
            "case {case}: expected the local reading of {resolved}, got {actual}"
        );
    }
}

/// A RESPONSE-STREAMING handler's answer is the bytes it WROTE, in the order it wrote
/// them. The stream is the answer, so one that has been begun cannot be withdrawn: a
/// handler that fails delivers what it had written and nothing more, with no error code on
/// the invocation, and its failure is reported in the run's log as the one `Invoke Error`
/// record the runtime writes for it. One row per way such a handler can end.
#[tokio::test]
async fn a_response_streaming_handlers_answer_is_what_it_wrote() {
    let cases: &[(&str, &str, &str, bool)] = &[
        (
            "a handler that writes and ends",
            "exports.handler = awslambda.streamifyResponse(async (event, stream) => { \
             stream.write('first\\n'); stream.write('second\\n'); stream.end(); });",
            "first\nsecond\n",
            false,
        ),
        (
            "a handler that throws before its first write",
            "exports.handler = awslambda.streamifyResponse(async () => { \
             throw new Error('threw before writing'); });",
            "",
            true,
        ),
        (
            "a handler that writes and then throws",
            "exports.handler = awslambda.streamifyResponse(async (event, stream) => { \
             stream.write('partial\\n'); throw new Error('threw after writing'); });",
            "partial\n",
            true,
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, source, expected, failed)) in cases.iter().enumerate() {
        let name = format!("streaming-{index}");
        create_function(&router, &name, &[("index.js", source)], json!({})).await;
        let (answered, log) = invoke_with_log(&router, &name).await;
        assert_eq!(
            answered.function_error(),
            None,
            "case {case}: a stream that has begun carries no error code on the invocation, \
             got {}",
            answered.text()
        );
        assert_eq!(
            answered.text(),
            *expected,
            "case {case}: expected exactly the bytes the handler wrote"
        );
        let records = log.matches("Invoke Error").count();
        assert_eq!(
            records,
            usize::from(*failed),
            "case {case}: expected {} Invoke Error record(s) in the log, got {records} in \
             {log:?}",
            usize::from(*failed)
        );
    }
}

/// One row of the failure table: the case, the handler's source, and the error type and
/// message the invocation is expected to report.
type FailureCase<'a> = (&'a str, &'a str, &'a str, &'a str);

/// The payload of a FAILED node invocation is exactly the three members a node runtime
/// reports, whose first trace line is the error's own line — one row per kind of value a
/// handler can fail with and per way of failing with it.
#[tokio::test]
async fn a_failed_node_invocation_reports_the_three_member_payload() {
    let cases: &[FailureCase<'_>] = &[
        (
            "a thrown Error subclass",
            "exports.handler = async () => { throw new RangeError('out of range'); };",
            "RangeError",
            "out of range",
        ),
        (
            "a rejected promise",
            "exports.handler = () => Promise.reject(new TypeError('rejected here'));",
            "TypeError",
            "rejected here",
        ),
        (
            "an error passed to the callback",
            "exports.handler = (event, context, callback) => { \
             callback(new Error('through the callback')); };",
            "Error",
            "through the callback",
        ),
        (
            "an error passed to context.fail",
            "exports.handler = (event, context) => { context.fail(new Error('failed here')); };",
            "Error",
            "failed here",
        ),
        (
            "an exception raised while the module itself loads",
            "throw new Error('init exploded');\nexports.handler = async () => 1;",
            "Error",
            "init exploded",
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, source, error_type, message)) in cases.iter().enumerate() {
        let name = format!("failure-{index}");
        create_function(&router, &name, &[("index.js", source)], json!({})).await;
        let answered = invoke(&router, &name, b"{}").await;
        assert_eq!(
            answered.status, 200,
            "case {case}: a handler that failed is still answered, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.function_error().as_deref(),
            Some("Unhandled"),
            "case {case}: the failure must be signalled, got {}",
            answered.text()
        );
        let document = answered.json();
        let payload = document
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected an error document, got {document}"));
        let mut members: Vec<&str> = payload.keys().map(String::as_str).collect();
        members.sort_unstable();
        assert_eq!(
            members,
            vec!["errorMessage", "errorType", "trace"],
            "case {case}: expected exactly the three members a node runtime reports, got \
             {members:?}"
        );
        assert_eq!(
            payload.get("errorType").and_then(Value::as_str),
            Some(*error_type),
            "case {case}: expected the thrown value's own class, got {document}"
        );
        assert_eq!(
            payload.get("errorMessage").and_then(Value::as_str),
            Some(*message),
            "case {case}: expected the thrown value's own message, got {document}"
        );
        let trace = payload
            .get("trace")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a trace, got {document}"));
        assert_eq!(
            trace.first().and_then(Value::as_str),
            Some(format!("{error_type}: {message}").as_str()),
            "case {case}: the trace's head is the error's own line, got {trace:?}"
        );
        assert!(
            trace.iter().skip(1).any(|frame| frame
                .as_str()
                .is_some_and(|line| line.contains("/var/task/index.js"))),
            "case {case}: a frame under the head must name the file the handler is in, got \
             {trace:?}"
        );
    }
}

/// A value that is NOT an error reports the TYPE of the value and the value as text, with
/// no trace at all — one row per kind of value a handler can throw that carries no stack.
#[tokio::test]
async fn a_thrown_value_that_is_no_error_reports_its_own_type() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("a string", "'thrown text'", "string", "thrown text"),
        ("a number", "42", "number", "42"),
        ("a plain object", "({ a: 1 })", "object", "[object Object]"),
        ("null", "null", "object", "null"),
        ("a boolean", "true", "boolean", "true"),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, thrown, error_type, message)) in cases.iter().enumerate() {
        let name = format!("thrown-{index}");
        let source = format!("exports.handler = async () => {{ throw {thrown}; }};");
        create_function(&router, &name, &[("index.js", source.as_str())], json!({})).await;
        let answered = invoke(&router, &name, b"{}").await;
        let actual = answered.json();
        assert_eq!(
            answered.function_error().as_deref(),
            Some("Unhandled"),
            "case {case}: the failure must be signalled, got {actual}"
        );
        assert_eq!(
            actual,
            json!({ "errorType": error_type, "errorMessage": message, "trace": [] }),
            "case {case}: expected the value's own type and text with no trace, got {actual}"
        );
    }
}

/// An invocation node cannot complete is reported as a FAILED one — one row per way a node
/// invocation ends without the handler answering.
#[tokio::test]
async fn an_invocation_node_cannot_complete_is_reported_as_failed() {
    let cases: &[(&str, Entries<'_>, Value, &str)] = &[
        (
            "module syntax in a CommonJS .js file",
            &[("index.js", "export const handler = async () => ({ ran: true });")],
            json!({}),
            "Runtime.UserCodeSyntaxError",
        ),
        (
            "a promise rejection nothing handled",
            &[(
                "index.js",
                "exports.handler = async () => { Promise.reject(new Error('stray')); \
                 return { ran: true }; };",
            )],
            json!({}),
            "Runtime.UnhandledPromiseRejection",
        ),
        (
            "a runtime option node does not know",
            &[("index.js", "exports.handler = async () => ({ ran: true });")],
            json!({ "Environment": { "Variables": { "NODE_OPTIONS": "--not-a-node-flag" } } }),
            "Runtime.NodeJsExit",
        ),
        (
            "a handler whose promise never settles",
            &[("index.js", "exports.handler = () => new Promise(() => {});")],
            json!({}),
            "Runtime.NodeJsExit",
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, entries, extra, error_type)) in cases.iter().enumerate() {
        let name = format!("unfinished-{index}");
        create_function(&router, &name, entries, extra.clone()).await;
        let answered = invoke(&router, &name, b"{}").await;
        let actual = answered.json();
        assert_eq!(
            answered.function_error().as_deref(),
            Some("Unhandled"),
            "case {case}: an invocation that cannot complete must be reported as failed, got \
             {actual}"
        );
        assert_eq!(
            actual.get("errorType").and_then(Value::as_str),
            Some(*error_type),
            "case {case}: expected the failure to be reported as {error_type}, got {actual}"
        );
    }
}

/// An ES module knows its own url, and `NODE_OPTIONS` is applied before the handler's
/// module is loaded: one row per place in the package an ES module can live, and one
/// invocation reading what the options set.
#[tokio::test]
async fn an_es_module_knows_its_url_and_node_options_is_applied() {
    let cases: &[(&str, Entries<'_>, &str, Value)] = &[
        (
            "an .mjs module at the package root",
            &[(
                "index.mjs",
                "export const handler = async () => ({ file: import.meta.url });",
            )],
            "index.handler",
            json!({ "file": "file:///var/task/index.mjs" }),
        ),
        (
            "an .mjs module in a nested directory",
            &[(
                "src/handlers/api.mjs",
                "export const handler = async () => ({ file: import.meta.url });",
            )],
            "src/handlers/api.handler",
            json!({ "file": "file:///var/task/src/handlers/api.mjs" }),
        ),
        (
            "a .js module of a package declared a module",
            &[
                ("package.json", r#"{"type":"module"}"#),
                (
                    "index.js",
                    "export const handler = async () => ({ file: import.meta.url, \
                     requireType: typeof require });",
                ),
            ],
            "index.handler",
            json!({ "file": "file:///var/task/index.js", "requireType": "undefined" }),
        ),
        (
            "a .js file winning over a same-named .mjs",
            &[
                ("index.js", "exports.handler = async () => ({ loaded: 'index.js' });"),
                (
                    "index.mjs",
                    "export const handler = async () => ({ loaded: 'index.mjs' });",
                ),
            ],
            "index.handler",
            json!({ "loaded": "index.js" }),
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (index, (case, entries, handler, expected)) in cases.iter().enumerate() {
        let name = format!("module-{index}");
        create_function(&router, &name, entries, json!({ "Handler": handler })).await;
        let answered = invoke(&router, &name, b"{}").await;
        assert_eq!(
            answered.function_error(),
            None,
            "case {case}: the invocation must report no function error, got {}",
            answered.text()
        );
        let actual = answered.json();
        assert_eq!(
            actual, *expected,
            "case {case}: expected {expected}, got {actual}"
        );
    }

    let applied: Entries<'_> = &[
        ("preload.js", "globalThis.preloaded = 'preloaded';"),
        (
            "index.js",
            "exports.handler = async () => ({ preloaded: globalThis.preloaded || null, \
             stackTraceLimit: Error.stackTraceLimit });",
        ),
    ];
    create_function(
        &router,
        "node-options",
        applied,
        json!({
            "Environment": {
                "Variables": {
                    "NODE_OPTIONS": "--require /var/task/preload.js --stack-trace-limit=3",
                },
            },
        }),
    )
    .await;
    let answered = invoke(&router, "node-options", b"{}").await;
    let actual = answered.json();
    assert_eq!(
        actual,
        json!({ "preloaded": "preloaded", "stackTraceLimit": 3 }),
        "expected the preloaded module and the stack depth NODE_OPTIONS set, got {actual}"
    );
}
