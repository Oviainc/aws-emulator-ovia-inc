// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a NODE function's invocation reports a handler it could not
//! load.
//!
//! The rule under test: a specifier a `require` cannot resolve fails the invocation as
//! `Runtime.ImportModuleError` carrying the node runtime's own text for it — the specifier
//! that was not found, then the REQUIRE STACK, the files that were requiring it innermost
//! first, ending at the runtime's own entry module — and a handler string that names
//! nothing callable fails it as `Runtime.HandlerNotFound`, which words the export the
//! module does not have apart from the export it has that is not a function. Either way
//! the invocation is an HTTP 200 carrying the error document and the `Unhandled` signal,
//! because the failure is the function's.
//!
//! Every row drives real HTTP requests through the router. The rows differ along what the
//! rule generalises over: whether the missing module is the handler's own or one the
//! package reaches for, how deep the require chain is when it fails, the specifier's form
//! (bare package, scoped package, relative path), and which way the handler string names
//! nothing callable.

use std::io::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::function_lifecycle::{
    create_function, make_activation_due, router_on, send, zip_file, Answer, ROLE,
};

/// Where `Invoke` is bound (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The task root a deployment package's files are mounted at, which is what a node stack
/// frame and a require stack name them by.
pub(crate) const TASK_ROOT: &str = "/var/task";

/// The runtime's own entry module, the outermost frame of every require stack: it is the
/// module that requires the handler's.
pub(crate) const RUNTIME_ENTRY: &str = "/var/runtime/index.mjs";

/// A .zip deployment package holding `entries`.
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

/// Create a `nodejs20.x` function holding `entries` with the handler `handler`, and bring
/// it to the state an invocation needs.
pub(crate) async fn create_node_function(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    entries: &[(&str, &str)],
    handler: &str,
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "nodejs20.x",
            "Handler": handler,
            "Timeout": 10,
            "MemorySize": 256,
            "Code": { "ZipFile": zip_file(&package(entries)) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// `Invoke` on `name`.
pub(crate) async fn invoke(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Answer {
    send(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        "application/octet-stream",
        b"{}".to_vec(),
        &[],
    )
    .await
}

/// The require stack text a node runtime appends to a `Cannot find module` message: the
/// requiring files, innermost first, one per line.
pub(crate) fn require_stack(frames: &[&str]) -> String {
    let listed: Vec<String> = frames.iter().map(|frame| format!("- {frame}")).collect();
    format!("\nRequire stack:\n{}", listed.join("\n"))
}

/// One row of the loader table: the package, the handler string, and the error document
/// the invocation must report.
struct LoaderCase {
    case: &'static str,
    entries: Vec<(&'static str, &'static str)>,
    handler: &'static str,
    error_type: &'static str,
    message: String,
}

/// The rows. Each `message` is built from the specifier and the file paths the package
/// really holds, never from what the emulator answered.
fn loader_cases() -> Vec<LoaderCase> {
    vec![
        LoaderCase {
            case: "a bare package the deployment package does not hold",
            entries: vec![(
                "index.js",
                "const held = require('left-pad');\nexports.handler = async () => held;\n",
            )],
            handler: "index.handler",
            error_type: "Runtime.ImportModuleError",
            message: format!(
                "Error: Cannot find module 'left-pad'{}",
                require_stack(&[&format!("{TASK_ROOT}/index.js"), RUNTIME_ENTRY])
            ),
        },
        LoaderCase {
            case: "a scoped package no runtime supplies",
            entries: vec![(
                "index.js",
                "require('@acme/absent-client');\nexports.handler = async () => 1;\n",
            )],
            handler: "index.handler",
            error_type: "Runtime.ImportModuleError",
            message: format!(
                "Error: Cannot find module '@acme/absent-client'{}",
                require_stack(&[&format!("{TASK_ROOT}/index.js"), RUNTIME_ENTRY])
            ),
        },
        LoaderCase {
            case: "a relative file the deployment package does not hold",
            entries: vec![("index.js", "require('./missing/helper');\n")],
            handler: "index.handler",
            error_type: "Runtime.ImportModuleError",
            message: format!(
                "Error: Cannot find module './missing/helper'{}",
                require_stack(&[&format!("{TASK_ROOT}/index.js"), RUNTIME_ENTRY])
            ),
        },
        LoaderCase {
            case: "a module two requires deep, which names the whole chain",
            entries: vec![
                ("index.js", "require('./lib/first');\n"),
                ("lib/first.js", "require('./second');\n"),
                ("lib/second.js", "require('absent-dependency');\n"),
            ],
            handler: "index.handler",
            error_type: "Runtime.ImportModuleError",
            message: format!(
                "Error: Cannot find module 'absent-dependency'{}",
                require_stack(&[
                    &format!("{TASK_ROOT}/lib/second.js"),
                    &format!("{TASK_ROOT}/lib/first.js"),
                    &format!("{TASK_ROOT}/index.js"),
                    RUNTIME_ENTRY,
                ])
            ),
        },
        LoaderCase {
            case: "the handler's OWN module, which only the runtime was requiring",
            entries: vec![("other.js", "exports.handler = async () => 1;\n")],
            handler: "index.handler",
            error_type: "Runtime.ImportModuleError",
            message: format!(
                "Error: Cannot find module 'index'{}",
                require_stack(&[RUNTIME_ENTRY])
            ),
        },
        LoaderCase {
            // A module that loaded keeps the `require` it was given, and so keeps the chain
            // it was loaded through: a specifier it cannot resolve LATER names that chain
            // and not whatever is loading at the time. The failure is the handler's own
            // error here rather than the runtime's loader class, because the module system
            // had already answered with the handler by then.
            case: "a require the handler makes while it runs",
            entries: vec![(
                "index.js",
                "exports.handler = async () => require('absent-at-call-time');\n",
            )],
            handler: "index.handler",
            error_type: "Error",
            message: format!(
                "Cannot find module 'absent-at-call-time'{}",
                require_stack(&[&format!("{TASK_ROOT}/index.js"), RUNTIME_ENTRY])
            ),
        },
        LoaderCase {
            case: "an export the handler's module does not have",
            entries: vec![("index.js", "exports.other = async () => 1;\n")],
            handler: "index.handler",
            error_type: "Runtime.HandlerNotFound",
            message: "index.handler is undefined or not exported".to_string(),
        },
        LoaderCase {
            case: "a nested export path the module does not reach",
            entries: vec![("index.js", "exports.nested = {};\n")],
            handler: "index.nested.deep.handler",
            error_type: "Runtime.HandlerNotFound",
            message: "index.nested.deep.handler is undefined or not exported".to_string(),
        },
        LoaderCase {
            case: "an export the module has that is not a function",
            entries: vec![("index.js", "exports.handler = 'not callable';\n")],
            handler: "index.handler",
            error_type: "Runtime.HandlerNotFound",
            message: "index.handler is not a function".to_string(),
        },
        LoaderCase {
            case: "a nested export that is not a function",
            entries: vec![("index.js", "exports.nested = { handler: 42 };\n")],
            handler: "index.nested.handler",
            error_type: "Runtime.HandlerNotFound",
            message: "index.nested.handler is not a function".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_handler_that_cannot_be_loaded_is_reported_the_way_the_node_runtime_reports_it() {
    for case in loader_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "node-loader";
        create_node_function(&router, &engine, function, &case.entries, case.handler).await;

        let answer = invoke(&router, function).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: a function error is still an HTTP 200, got {} {}",
            case.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.header("x-amz-function-error").as_deref(),
            Some("Unhandled"),
            "case {:?}: expected the Unhandled signal, got {:?}",
            case.case,
            answer.header("x-amz-function-error")
        );
        let document = answer.json();
        assert_eq!(
            document.get("errorType").and_then(Value::as_str),
            Some(case.error_type),
            "case {:?}: expected the error type {}, got {document}",
            case.case,
            case.error_type
        );
        assert_eq!(
            document.get("errorMessage").and_then(Value::as_str),
            Some(case.message.as_str()),
            "case {:?}: expected the message {:?}, got {document}",
            case.case,
            case.message
        );
        // The trace a node runtime reports opens with the "<errorType>: <errorMessage>"
        // line, which is how a client reading the log sees the same failure. A trace is a
        // stack TEXT reported one entry per LINE, so a message spanning several lines — a
        // require stack does — opens the trace with one entry per line of it
        // ([`crate::node_error_trace_lines`] holds that rule over its own table).
        let head = format!("{}: {}", case.error_type, case.message);
        let opened: Vec<&str> = document
            .get("trace")
            .and_then(Value::as_array)
            .map(|frames| {
                frames
                    .iter()
                    .filter_map(Value::as_str)
                    .take(head.split('\n').count())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            opened,
            head.split('\n').collect::<Vec<&str>>(),
            "case {:?}: expected the trace to open with the lines of the error line, got \
             {document}",
            case.case
        );
    }
}
