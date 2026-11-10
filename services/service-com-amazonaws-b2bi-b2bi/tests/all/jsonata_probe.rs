// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that probe the JSONata engine with mapping templates it will not
//! compile, and assert the WORDING `TestMapping` reports its refusal with.
//!
//! The rule under test: a `mappingTemplate` the JSONata engine refuses is answered with
//! the modelled `ValidationException` (HTTP 400) whose message is the fixed clause
//! `JSONata error. Failed to map input file:`, then the ENGINE's own diagnosis of the
//! expression — the sentence the JSONata error catalogue states, with the token it names
//! in double quotes — and then the span of the expression the diagnosis is about, as
//! `From line <l> column <c> to line <l> column <c>.`. The message names no input member
//! and carries no engine-internal error code.
//!
//! The rows differ along what the rule generalises over: which token the expression still
//! needed (a parenthesis, a bracket, a brace, or none at all, read off the marks the
//! caller left open), whether the parse stopped at the end of the expression or on a
//! character inside it, where the expression begins (so the span's start moves), and how
//! many lines it spans.
//!
//! Every row is driven over the wire (awsJson1_0, `x-amz-target: B2BI.TestMapping`)
//! through the same router the emulator boots.

use serde_json::{json, Value};

use crate::onboarding_loop::{b2bi_router, call, error_code, text};

/// The fixed clause every one of these refusals begins with.
const JSONATA_FAILURE: &str = "JSONata error. Failed to map input file:";

/// One row: what a failing assertion calls it, the template, the sentence the refusal
/// states, and the span (from, to) it is reported over.
type DiagnosisRow = (
    &'static str,
    &'static str,
    &'static str,
    ((usize, usize), (usize, usize)),
);

/// RULE: see the module docs — the fixed clause, the engine's own sentence, and the span
/// of the expression it is about.
#[tokio::test]
async fn a_template_the_engine_refuses_is_reported_with_the_engines_own_diagnosis() {
    let router = b2bi_router();

    // (case, the template, the engine's sentence, the span it is reported over)
    let rows: Vec<DiagnosisRow> = vec![
        (
            "a function invocation left open",
            "$join(",
            r#"Expected ")" before end of expression."#,
            ((1, 2), (1, 7)),
        ),
        (
            "a function invocation whose argument list is left open",
            "$join(items",
            r#"Expected ")" before end of expression."#,
            ((1, 2), (1, 12)),
        ),
        (
            "an array constructor left open",
            "[1,2",
            r#"Expected "]" before end of expression."#,
            ((1, 1), (1, 5)),
        ),
        (
            "an object constructor left open",
            r#"{"total":1"#,
            r#"Expected "}" before end of expression."#,
            ((1, 1), (1, 11)),
        ),
        (
            "a block left open",
            "(1+2",
            r#"Expected ")" before end of expression."#,
            ((1, 1), (1, 5)),
        ),
        (
            // The awaited token is read off the EXPRESSION: here the innermost mark the
            // caller left open is a bracket, so a bracket is what the sentence names.
            "an array constructor left open after a comma",
            "[1,",
            r#"Expected "]" before end of expression."#,
            ((1, 1), (1, 4)),
        ),
        (
            "an object constructor left open after a key",
            r#"{"total":"#,
            r#"Expected "}" before end of expression."#,
            ((1, 1), (1, 10)),
        ),
        (
            // Nothing was left open, so no token was awaited and the engine's own
            // sentence stands.
            "an operator with nothing after it",
            "1 +",
            r#"The symbol "(end)" cannot be used as a unary operator."#,
            ((1, 1), (1, 4)),
        ),
        (
            "a close marker with nothing left to close",
            "$join(a))",
            r#"Syntax error ")"."#,
            ((1, 2), (1, 9)),
        ),
        (
            // The engine reports its position in BYTES, so a template carrying a
            // multi-byte character spans more columns than it has characters — and no
            // position may split one.
            "an expression carrying a multi-byte character",
            "($x = \"é\"",
            r#"Expected ")" before end of expression."#,
            ((1, 1), (1, 11)),
        ),
        (
            // The span starts at the expression's first token, so leading whitespace and
            // a second line move it.
            "an expression that begins on its second line",
            "\n  $join(",
            r#"Expected ")" before end of expression."#,
            ((2, 4), (2, 9)),
        ),
    ];

    for (case, template, sentence, (from, to)) in rows {
        let (status, body) = call(
            &router,
            "TestMapping",
            json!({
                "fileFormat": "JSON",
                "inputFileContent": r#"{"items":[1,2],"total":3}"#,
                "mappingTemplate": template,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let expected = format!(
            "{JSONATA_FAILURE} {sentence} From line {} column {} to line {} column {}.",
            from.0, from.1, to.0, to.1
        );
        // `ValidationException$Message` has a capital M in this service's model.
        let message = text(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains("mappingTemplate"),
            "case {case:?}: the engine's diagnosis names no input member, got {message:?}"
        );
        assert!(
            !message.contains('`'),
            "case {case:?}: a token the diagnosis names is stated in double quotes, got \
             {message:?}"
        );
    }
}

/// A template the engine COMPILES is evaluated, so the refusal above is about the
/// expression and not about every template: the control that keeps the table above honest.
#[tokio::test]
async fn a_template_the_engine_compiles_is_evaluated() {
    let router = b2bi_router();
    let (status, body): (u16, Value) = call(
        &router,
        "TestMapping",
        json!({
            "fileFormat": "JSON",
            "inputFileContent": r#"{"items":[1,2],"total":3}"#,
            "mappingTemplate": "$sum(items)",
        }),
    )
    .await;
    assert_eq!(status, 200, "a valid template must be served, got {status} {body}");
    // The member carries the mapped file's TEXT, JSON-string-encoded, so a client parses
    // the member once for the text (`test_mapping.rs` is where that encoding is asserted).
    let member = text(&body, "mappedFileContent");
    let mapped: String = serde_json::from_str(&member)
        .unwrap_or_else(|failure| panic!("mappedFileContent {member:?} must hold text: {failure}"));
    assert_eq!(
        mapped, "3",
        "the template must be evaluated over the document, got {mapped:?}"
    );
}
