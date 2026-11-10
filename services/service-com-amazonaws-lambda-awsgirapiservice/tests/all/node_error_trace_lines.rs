// SPDX-License-Identifier: CC0-1.0

//! What a NODE function's error document reports as its `trace`.
//!
//! The rule under test: a node error document's `trace` is the failure's stack TEXT reported
//! one entry per LINE — the `"<errorType>: <errorMessage>"` line the stack opens with, then
//! the frames under it — so no entry ever carries a line break, and a message that spans
//! several lines (the require stack a `Cannot find module` message names) opens the trace
//! with one entry per line of itself rather than with one entry holding the whole message.
//!
//! The rows differ along what the rule generalises over: how many lines the message spans
//! (one, and three to five), which failure class reports it (the handler's own error, the
//! runtime's loader classes, an unhandled rejection the runtime words itself), and whether
//! the failure happened while the module system was loading the handler or while the handler
//! was running.
//!
//! The package, function and invocation helpers are [`crate::node_loader_errors`]'s, reused
//! rather than copied.

use std::sync::Arc;

use serde_json::Value;

use crate::function_lifecycle::router_on;
use crate::node_loader_errors::{
    create_node_function, invoke, require_stack, RUNTIME_ENTRY, TASK_ROOT,
};

/// One row: the package, the handler string, and the error document the invocation reports.
struct TraceCase {
    case: &'static str,
    entries: Vec<(&'static str, &'static str)>,
    handler: &'static str,
    error_type: &'static str,
    message: String,
    /// How many LINES the reported error line spans, stated by the row so the count is
    /// checked against the rule and not merely re-derived from the answer.
    head_lines: usize,
}

/// The rows.
fn trace_cases() -> Vec<TraceCase> {
    vec![
        TraceCase {
            case: "a single-line error the handler threw",
            entries: vec![(
                "index.js",
                "exports.handler = async () => { throw new Error('boom'); };\n",
            )],
            handler: "index.handler",
            error_type: "Error",
            message: "boom".to_string(),
            head_lines: 1,
        },
        TraceCase {
            case: "a single-line message the runtime words itself",
            entries: vec![("index.js", "exports.other = async () => 1;\n")],
            handler: "index.handler",
            error_type: "Runtime.HandlerNotFound",
            message: "index.handler is undefined or not exported".to_string(),
            head_lines: 1,
        },
        TraceCase {
            case: "the handler's own module missing, a two-line require stack",
            entries: vec![("other.js", "exports.handler = async () => 1;\n")],
            handler: "index.handler",
            error_type: "Runtime.ImportModuleError",
            message: format!(
                "Error: Cannot find module 'index'{}",
                require_stack(&[RUNTIME_ENTRY])
            ),
            head_lines: 3,
        },
        TraceCase {
            case: "a bare package missing while the handler's module loads",
            entries: vec![(
                "index.js",
                "require('left-pad');\nexports.handler = async () => 1;\n",
            )],
            handler: "index.handler",
            error_type: "Runtime.ImportModuleError",
            message: format!(
                "Error: Cannot find module 'left-pad'{}",
                require_stack(&[&format!("{TASK_ROOT}/index.js"), RUNTIME_ENTRY])
            ),
            head_lines: 4,
        },
        TraceCase {
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
            head_lines: 4,
        },
        TraceCase {
            case: "a require a loaded module makes, naming the chain it was loaded through",
            entries: vec![
                (
                    "index.js",
                    "const helper = require('./lib/helper');\n\
                     exports.handler = async () => helper();\n",
                ),
                (
                    "lib/helper.js",
                    "module.exports = function () { return require('absent-dependency'); };\n",
                ),
            ],
            handler: "index.handler",
            error_type: "Error",
            message: format!(
                "Cannot find module 'absent-dependency'{}",
                require_stack(&[
                    &format!("{TASK_ROOT}/lib/helper.js"),
                    &format!("{TASK_ROOT}/index.js"),
                    RUNTIME_ENTRY,
                ])
            ),
            head_lines: 5,
        },
        TraceCase {
            case: "a rejection nothing handled, whose reason names a require stack",
            entries: vec![(
                "index.js",
                "exports.handler = async () => {\n\
                 Promise.resolve().then(function () { require('absent-rejected'); });\n\
                 return 'done';\n\
                 };\n",
            )],
            handler: "index.handler",
            error_type: "Runtime.UnhandledPromiseRejection",
            message: format!(
                "Error: Cannot find module 'absent-rejected'{}",
                require_stack(&[&format!("{TASK_ROOT}/index.js"), RUNTIME_ENTRY])
            ),
            head_lines: 4,
        },
    ]
}

#[tokio::test]
async fn a_node_error_document_reports_its_trace_one_entry_per_line() {
    for (index, case) in trace_cases().into_iter().enumerate() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = format!("node-trace-{index}");
        create_node_function(&router, &engine, &function, &case.entries, case.handler).await;

        let answer = invoke(&router, &function).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: a function error is still an HTTP 200, got {} {}",
            case.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
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

        let trace: Vec<&str> = document
            .get("trace")
            .and_then(Value::as_array)
            .map(|frames| frames.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert!(
            !trace.is_empty(),
            "case {:?}: expected a trace, got {document}",
            case.case
        );
        // The rule itself: a trace entry is ONE line of the stack text.
        for entry in &trace {
            assert!(
                !entry.contains('\n'),
                "case {:?}: a trace entry is one line of the stack, got {entry:?} in {trace:?}",
                case.case
            );
        }
        let head_line = format!("{}: {}", case.error_type, case.message);
        let head: Vec<&str> = head_line.split('\n').collect();
        assert_eq!(
            head.len(),
            case.head_lines,
            "case {:?}: expected the error line to span {} line(s), got {head:?}",
            case.case,
            case.head_lines
        );
        assert_eq!(
            &trace[..head.len().min(trace.len())],
            head.as_slice(),
            "case {:?}: expected the trace to open with the lines of the error line, got \
             {trace:?}",
            case.case
        );
        // The require stack is stated ONCE: a class the runtime words itself replaces the
        // reason's error line rather than repeating it.
        assert_eq!(
            trace.iter().filter(|entry| **entry == "Require stack:").count(),
            usize::from(case.message.contains("Require stack:")),
            "case {:?}: expected the require stack to be stated once at most, got {trace:?}",
            case.case
        );
    }
}
