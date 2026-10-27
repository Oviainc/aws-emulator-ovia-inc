// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DIAGNOSTIC this service quotes for a document it could
//! not read as JSON, driven over its own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes in `$.message`.
//!
//! `client_error_wrappers` owns the WRAPPER a refusal puts around that diagnostic
//! (`Invalid State Machine Execution Input: '<diagnostic>'`); this suite owns the
//! diagnostic itself, under one rule:
//!
//! > a refusal about a document the request carried names WHERE that document
//! > stopped parsing — the unquoted word the parse stopped on where a value belongs,
//! > together with the classes a value could have begun with, or else the character
//! > it stopped on, or the end of the input, and what was expected there — never the
//! > position a parser's cursor stopped at, which names none of those.
//!
//! The rows differ along the KIND of fault — a bare word, a non-ASCII word, an
//! unquoted field name, a missing colon, a missing comma in each container, a missing
//! value, a trailing comma in each container, a truncated object, array and string,
//! a document with no content, a stray quote character, a character past the first
//! 256 code points, a malformed number, an unrecognized escape, a second document in
//! the payload — and the rule is asserted on every operation that reads a caller's
//! document, not only on the one that exposed it. Rows whose fault class this service
//! does not reword assert the READING (where the parse stopped) rather than the
//! parser's own clause for it. The last test is the control: a document that IS JSON
//! is accepted, so the table separates "refused and reported" from "refused at all".
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` already builds and `failure` is `error_messages`' own;
//! `tests/all.rs` is this binary's crate root, so both are reached through their
//! sibling suites.

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    json_parse_diagnostic, INVALID_STATE_MACHINE_DEFINITION, INVALID_STATE_MACHINE_EXECUTION_INPUT,
};

use crate::error_messages::failure;
use crate::state_machine_lifecycle::{create_machine, string, Emulator, HELLO_DEFINITION, ROLE_ARN};
use crate::sync_execution::create_express;

/// A definition that is one state, which is what `TestState` reads when the request
/// names no `stateName`.
const PASS_STATE: &str = r#"{"Type":"Pass","Result":{"ok":true},"End":true}"#;

/// The token classes a JSON value may begin with, as a diagnostic lists them.
///
/// Spelled literally here rather than read off the service: the recorded refusal for
/// the input `not-json` is
/// `Invalid State Machine Execution Input: 'Unrecognized token 'not': was expecting
/// (JSON String, Number, Array, Object or token 'null', 'true' or 'false')'`, so
/// this suite fails if the list is reworded.
const VALUE_CLASSES: &str = "JSON String, Number, Array, Object or token 'null', 'true' or 'false'";

/// What a row requires of the diagnostic.
enum Expected {
    /// The whole diagnostic, spelled out.
    Exact(String),
    /// The diagnostic names WHERE the parse stopped — the character it stopped on,
    /// or the end of the input — for a fault class this service reports in the
    /// parser's own clause rather than in words of its own.
    NamesWhereItStopped,
}

/// The diagnostic for a bare word where a value was expected.
fn unrecognized_token(token: &str) -> Expected {
    Expected::Exact(format!(
        "Unrecognized token '{token}': was expecting ({VALUE_CLASSES})"
    ))
}

/// The diagnostic for a character where a value was expected.
fn not_a_value(character: char) -> Expected {
    Expected::Exact(format!(
        "Unexpected character ({}): was expecting a valid value ({VALUE_CLASSES})",
        named(character)
    ))
}

/// The diagnostic for `character` carrying the clause `expected`.
fn unexpected(character: char, expected: &str) -> Expected {
    Expected::Exact(format!("Unexpected character ({}): {expected}", named(character)))
}

/// The diagnostic for a document that ended before it was complete.
fn ended_early(expected: &str) -> Expected {
    Expected::Exact(format!("Unexpected end-of-input: {expected}"))
}

/// How a diagnostic names one character: the character and its character code.
fn named(character: char) -> String {
    format!("'{character}' (code {})", u32::from(character))
}

/// Every row: what the fault is, the document that carries it, and what the service
/// must quote for it.
///
/// The documents are all inside the modelled length of the members they are sent as
/// (`input` is 0..262144), so each is refused for its CONTENT and not by a boundary.
fn faults() -> Vec<(&'static str, &'static str, Expected)> {
    vec![
        // The recorded row: `not-json` is refused naming `not`, the whole word the
        // parse stopped on, and not the whole member value.
        ("a bare word", "not-json", unrecognized_token("not")),
        (
            "a bare word written outside ASCII",
            "ünicode",
            unrecognized_token("ünicode"),
        ),
        (
            "a word a template left unsubstituted",
            r#"{"a":$value}"#,
            unrecognized_token("$value"),
        ),
        (
            "a literal spelled short",
            r#"{"a":tru}"#,
            unrecognized_token("tru"),
        ),
        (
            "an unquoted field name",
            r#"{oops: 1}"#,
            unexpected('o', "was expecting double-quote to start field name"),
        ),
        (
            "a field name with no colon after it",
            r#"{"a" 1}"#,
            unexpected(
                '1',
                "was expecting a colon to separate field name and value",
            ),
        ),
        (
            "two object entries with no comma between them",
            r#"{"a":1 "b":2}"#,
            unexpected('"', "was expecting comma to separate Object entries"),
        ),
        (
            "two array elements with no comma between them",
            "[1 2]",
            unexpected('2', "was expecting comma to separate Array entries"),
        ),
        ("an entry with no value", r#"{"a":}"#, not_a_value('}')),
        (
            "a trailing comma in an object",
            r#"{"a":1,}"#,
            unexpected('}', "was expecting double-quote to start field name"),
        ),
        ("a trailing comma in an array", "[1,]", not_a_value(']')),
        (
            "a truncated object",
            r#"{"a":1"#,
            ended_early("expected close marker for Object"),
        ),
        (
            "a truncated array",
            "[1,2",
            ended_early("expected close marker for Array"),
        ),
        (
            "a truncated string",
            r#"{"a":"text"#,
            ended_early("expected close marker for String"),
        ),
        (
            "a single-quoted string, which JSON does not have",
            "'text'",
            not_a_value('\''),
        ),
        ("the empty document", "", ended_early("expected a value")),
        (
            "a document of whitespace alone",
            "   ",
            ended_early("expected a value"),
        ),
        (
            // A character outside ASCII that begins no word: the diagnostic names it
            // and its code, not the byte the parse stopped on.
            "a currency sign where a value was expected",
            "€",
            not_a_value('€'),
        ),
        (
            "two documents in one payload",
            "{} {}",
            unexpected('{', "was expecting end of the document"),
        ),
        // Fault classes the parser reports in its own clause: the reading asserted
        // is WHERE the document stopped, which is what no cursor position names.
        (
            "a literal that continues into more word characters",
            "nullish",
            Expected::NamesWhereItStopped,
        ),
        (
            "a number with a redundant leading zero",
            "01",
            Expected::NamesWhereItStopped,
        ),
        (
            "a minus sign with no digit after it",
            "-x",
            Expected::NamesWhereItStopped,
        ),
        (
            "a decimal point with no digit after it",
            "[1.,2]",
            Expected::NamesWhereItStopped,
        ),
        (
            "an escape the language does not declare",
            r#"{"a":"\q"}"#,
            Expected::NamesWhereItStopped,
        ),
        (
            "an unescaped line feed inside a string",
            "{\"a\":\"one\ntwo\"}",
            Expected::NamesWhereItStopped,
        ),
    ]
}

/// The value a message quotes inside `<condition>: '<value>'`.
fn quoted_value<'a>(case: &str, message: &'a str, condition: &str) -> &'a str {
    message
        .strip_prefix(&format!("{condition}: '"))
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a message of the form \"{condition}: '<diagnostic>'\", \
                 got {message:?}"
            )
        })
}

/// A parser's CURSOR — the position it stopped at — names neither where the document
/// stopped nor what was expected there, so no message may carry one.
fn assert_no_cursor(case: &str, message: &str) {
    for cursor in ["line ", "column "] {
        assert!(
            !message.contains(cursor),
            "case {case:?}: expected the diagnostic to name where the document stopped rather \
             than a parser cursor, got {message:?} (carrying {cursor:?})"
        );
    }
}

/// The diagnostic `document` must carry, as this service's ONE constructor words it
/// from the parse that refuses the document.
fn constructed(case: &str, document: &str) -> String {
    let error = serde_json::from_str::<Value>(document)
        .expect_err(&format!("case {case:?}: the document must not parse"));
    json_parse_diagnostic(document, &error)
}

/// Judge one diagnostic against what its row requires.
fn assert_diagnostic(case: &str, document: &str, expected: &Expected, diagnostic: &str) {
    match expected {
        Expected::Exact(text) => assert_eq!(
            diagnostic, text,
            "case {case:?}: expected the diagnostic {text:?}, got {diagnostic:?}"
        ),
        Expected::NamesWhereItStopped => assert!(
            diagnostic.starts_with("Unexpected character ('")
                || diagnostic.starts_with("Unexpected end-of-input: "),
            "case {case:?}: expected the diagnostic to name where the document stopped, got \
             {diagnostic:?}"
        ),
    }
    // One construction answers for every document this service reads, so a refusal
    // cannot drift from it.
    assert_eq!(
        diagnostic,
        constructed(case, document),
        "case {case:?}: expected the refusal to read the service's one constructor, got \
         {diagnostic:?}"
    );
}

/// An execution `input` that is not JSON is refused with the diagnostic for the
/// document, on every operation that reads one.
#[tokio::test]
async fn a_refused_input_names_where_the_document_stopped_parsing() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "diag-input", HELLO_DEFINITION).await;
    // The SYNCHRONOUS start reads the same member, and serves EXPRESS workflows
    // only, so it needs a state machine of that type to be refused for its input.
    let express = create_express(&emulator, "diag-input-express", HELLO_DEFINITION).await;

    for (kind, document, expected) in faults() {
        // `StartExecution`, the operation the rule was reported against.
        let case = format!("StartExecution with {kind} as its input");
        let (status, code, message) = failure(
            &emulator,
            &case,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "input": document }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExecutionInput"),
            "case {case:?}: expected 400 InvalidExecutionInput, got {status} {code} with message \
             {message:?}"
        );
        assert_no_cursor(&case, &message);
        let detail = quoted_value(&case, &message, INVALID_STATE_MACHINE_EXECUTION_INPUT);
        assert_diagnostic(&case, document, &expected, detail);

        // `TestState` reads a caller's document as the input a state is tested on,
        // and refuses it in the same words: the rule belongs to the document, not
        // to one operation.
        let case = format!("TestState with {kind} as its input");
        let (status, code, message) = failure(
            &emulator,
            &case,
            "TestState",
            &json!({ "definition": PASS_STATE, "roleArn": ROLE_ARN, "input": document }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExecutionInput"),
            "case {case:?}: expected 400 InvalidExecutionInput, got {status} {code} with message \
             {message:?}"
        );
        assert_no_cursor(&case, &message);
        let detail = quoted_value(&case, &message, INVALID_STATE_MACHINE_EXECUTION_INPUT);
        assert_diagnostic(&case, document, &expected, detail);

        // `StartSyncExecution` reads the same member of the same API, and refuses
        // it in the same words.
        let case = format!("StartSyncExecution with {kind} as its input");
        let (status, code, message) = failure(
            &emulator,
            &case,
            "StartSyncExecution",
            &json!({ "stateMachineArn": express, "input": document }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExecutionInput"),
            "case {case:?}: expected 400 InvalidExecutionInput, got {status} {code} with message \
             {message:?}"
        );
        assert_no_cursor(&case, &message);
        let detail = quoted_value(&case, &message, INVALID_STATE_MACHINE_EXECUTION_INPUT);
        assert_diagnostic(&case, document, &expected, detail);
    }
}

/// A state machine `definition` that is not JSON carries the SAME diagnostic, on
/// both operations that refuse one and in the validation report.
#[tokio::test]
async fn a_refused_definition_carries_the_same_diagnostic() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "diag-definition", HELLO_DEFINITION).await;

    for (index, (kind, document, _)) in faults().into_iter().enumerate() {
        // A definition that is empty or blank is outside the modelled length
        // (1..1048576), or carries nothing to parse at all, so what refuses it is
        // the boundary rather than this rule.
        if document.trim().is_empty() {
            continue;
        }
        let diagnostic = constructed(kind, document);

        for (operation, body) in [
            (
                "CreateStateMachine",
                json!({
                    "name": format!("diag-definition-{index}"),
                    "definition": document,
                    "roleArn": ROLE_ARN,
                }),
            ),
            (
                "UpdateStateMachine",
                json!({ "stateMachineArn": machine, "definition": document }),
            ),
        ] {
            let case = format!("{operation} with {kind} as its definition");
            let (status, code, message) = failure(&emulator, &case, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidDefinition"),
                "case {case:?}: expected 400 InvalidDefinition, got {status} {code} with message \
                 {message:?}"
            );
            assert_no_cursor(&case, &message);
            let detail = quoted_value(&case, &message, INVALID_STATE_MACHINE_DEFINITION);
            assert!(
                detail.ends_with(&diagnostic),
                "case {case:?}: expected the diagnostic to end with {diagnostic:?}, got \
                 {detail:?}"
            );
        }

        // `ValidateStateMachineDefinition` reports the same problem as a diagnostic
        // of its own rather than as a refusal, and reads the same construction.
        let case = format!("ValidateStateMachineDefinition with {kind}");
        let reported = emulator
            .ok(
                "ValidateStateMachineDefinition",
                &json!({ "definition": document }),
            )
            .await;
        assert_eq!(
            reported.get("result").and_then(Value::as_str),
            Some("FAIL"),
            "case {case:?}: expected a FAIL result, got {reported:?}"
        );
        let diagnostics = reported
            .get("diagnostics")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected diagnostics in {reported:?}"));
        let found: Vec<&str> = diagnostics
            .iter()
            .filter_map(|reported| reported.get("message").and_then(Value::as_str))
            .collect();
        assert!(
            found.iter().any(|message| message.ends_with(&diagnostic)),
            "case {case:?}: expected a diagnostic ending with {diagnostic:?}, got {found:?}"
        );
        for message in found {
            assert_no_cursor(&case, message);
        }
    }
}

/// The diagnostic stays bounded for a document built to make it grow: a word as long
/// as the member may carry is CLIPPED rather than echoed whole, and a document nested
/// past what the parse reads is still reported as where it stopped.
///
/// A refusal is built from a caller's document, so neither may turn one request into
/// a much larger response.
#[tokio::test]
async fn the_diagnostic_stays_bounded_for_a_hostile_document() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "diag-bounds", HELLO_DEFINITION).await;

    // (case, the document, what the diagnostic must open with)
    let cases = [
        (
            "a bare word as long as the member carries",
            "a".repeat(200_000),
            "Unrecognized token 'aaa",
        ),
        (
            "a document nested past what the parse reads",
            "[".repeat(1024),
            "Unexpected character ('[' (code 91))",
        ),
    ];

    for (case, document, opening) in cases {
        let (status, code, message) = failure(
            &emulator,
            case,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "input": document }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExecutionInput"),
            "case {case:?}: expected 400 InvalidExecutionInput, got {status} {code}"
        );
        let detail = quoted_value(case, &message, INVALID_STATE_MACHINE_EXECUTION_INPUT);
        assert!(
            detail.starts_with(opening),
            "case {case:?}: expected a diagnostic opening {opening:?}, got {detail:?}"
        );
        assert!(
            message.len() < 300,
            "case {case:?}: expected the refusal to stay bounded, got {} characters: {message:?}",
            message.len()
        );
        assert_no_cursor(case, &message);
    }
}

/// The control: a document that IS JSON is read, whatever form it takes, so the
/// rule above is about documents the service cannot read and not about refusing
/// documents.
#[tokio::test]
async fn a_document_that_is_json_is_read() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "diag-control", HELLO_DEFINITION).await;

    // Rows differ along the JSON form: each of the language's value kinds, a
    // number with a sign, a fraction and an exponent, a string carrying the
    // escapes the fault table malforms, nesting, and whitespace around the whole
    // document.
    let documents = [
        ("an object", r#"{"a":1}"#),
        ("the empty object", "{}"),
        ("an array", "[1,2,3]"),
        ("the empty array", "[]"),
        ("a string", r#""text""#),
        ("an integer", "0"),
        ("a signed number with a fraction and an exponent", "-1.5e+3"),
        ("the null literal", "null"),
        ("the true literal", "true"),
        ("nesting", r#"{"nested":{"deep":[{"ok":true}]}}"#),
        ("a string carrying escapes", r#"{"escaped":"a\"b\u00e9\n"}"#),
        ("a document padded with whitespace", "  {\"padded\":1}\n"),
    ];

    for (index, (kind, document)) in documents.iter().enumerate() {
        let case = format!("StartExecution with {kind} as its input");
        let started = emulator
            .ok(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": format!("diag-control-{index}"),
                    "input": document,
                }),
            )
            .await;
        let arn = string(&started, "executionArn");
        assert!(
            arn.contains(":execution:"),
            "case {case:?}: expected an execution ARN, got {arn:?}"
        );
    }
}
