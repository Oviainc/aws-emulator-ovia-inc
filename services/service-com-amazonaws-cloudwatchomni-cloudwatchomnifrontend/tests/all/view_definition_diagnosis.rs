// SPDX-License-Identifier: CC0-1.0

//! What a refused view `definition` is told about itself.
//!
//! A definition that is not one SQL query is refused with the modelled
//! `ValidationException` (HTTP 400), and the message it carries is the SQL parser's own
//! reading of the submitted text under one fixed prefix: `Invalid view definition: `
//! followed verbatim by the parser's error, itself opening `sql parser error: ` and then
//! naming what the parser expected, the token it stopped on, and the 1-based line and
//! column that token sits at.
//!
//! The rule this suite asserts is that the diagnosis is a function of the SUBMITTED TEXT,
//! so the rows vary that text:
//!
//! * the same defect moved around the definition must be reported where it stands — the
//!   positions below are computed from how each row is BUILT, not read off any one run;
//! * different defects must earn different diagnoses, each naming the token it stopped
//!   on, rather than one constant sentence for every refusal;
//! * the refusal that travels over the rpcv2Cbor wire is the same message, so a real
//!   client reads the diagnosis and not a generality.
//!
//! The harness is `view_lifecycle`'s: one shared engine behind the real handlers.

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_view::CreateViewErrorModeled;
use omni::ops::types::CREATE_VIEW_INPUT_SCHEMA;
use omni::scaffolded::Scaffolded;
use omni::state::{view_definition_rejection, INVALID_VIEW_DEFINITION};

use super::view_lifecycle::{create_input, view_name, Omni};

/// The fixed text the parser's own error rendering opens with, so a refusal message is
/// the service's prefix followed by the parser's error verbatim.
const PARSER_ERROR: &str = "sql parser error: ";

/// The expectation the parser reports for text that carries no statement, as the
/// recorded refusal of a definition that is not SQL words it.
const EXPECTED_A_STATEMENT: &str = "Expected: an SQL statement, found: ";

/// A definition whose only defect is a brace where a statement has to start, placed at
/// `line` and `column` BY CONSTRUCTION: on the first line the brace is all there is, and
/// on any later line it follows a complete statement, so the parser stops on the brace
/// rather than on anything before it.
fn brace_at(line: usize, column: usize) -> String {
    let indent = " ".repeat(column - 1);
    if line == 1 {
        format!("{indent}{{}}")
    } else {
        format!("SELECT 1;{}{indent}{{}}", "\n".repeat(line - 1))
    }
}

/// The message a definition whose brace sits at `line`/`column` must be refused with:
/// the service's fixed prefix, the parser's fixed prefix, the parser's expectation and
/// the offending token, then the position that token was BUILT at.
fn expected_brace_message(line: usize, column: usize) -> String {
    format!("{INVALID_VIEW_DEFINITION}{PARSER_ERROR}{EXPECTED_A_STATEMENT}{{ at Line: {line}, Column: {column}")
}

/// The message the operation's modelled `ValidationException` carries for `definition`.
async fn refusal_message(omni: &Omni, case: &str, name: &str, definition: &str) -> String {
    match omni.create_view(&create_input(name, definition)).await {
        Ok(output) => panic!("case {case:?}: expected a ValidationException, got Ok({output:?})"),
        Err(Scaffolded::Modeled(CreateViewErrorModeled::ValidationException(error))) => {
            error.message
        }
        Err(other) => {
            panic!("case {case:?}: expected the modelled ValidationException, got {other:?}")
        }
    }
}

#[tokio::test]
async fn the_refusal_reports_the_parser_diagnosis_where_the_defect_stands() {
    let omni = Omni::new();
    // The first row is the recorded case: the smallest definition that is not SQL at
    // all, whose defect sits at line 1, column 1. The rest move the SAME defect down and
    // across the text, so what varies between rows is only where the parser stops.
    let positions = [(1usize, 1usize), (1, 5), (2, 1), (3, 4), (6, 12)];
    let mut seen: Vec<String> = Vec::new();
    for (line, column) in positions {
        let case = format!("a brace at line {line}, column {column}");
        let definition = brace_at(line, column);
        let expected = expected_brace_message(line, column);

        let rejection = view_definition_rejection(&definition);
        assert_eq!(
            rejection.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: the definition rule must report the parser's diagnosis, got {rejection:?}"
        );

        let name = view_name(&format!("brace-{line}-{column}"));
        let message = refusal_message(&omni, &case, &name, &definition).await;
        assert_eq!(
            message, expected,
            "case {case:?}: CreateView must refuse with the parser's diagnosis, got {message:?}"
        );
        seen.push(message);
    }
    // One defect at five places is five diagnoses: the position is carried, not dropped.
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        positions.len(),
        "the same defect at {} places must earn {} distinct diagnoses, got {seen:?}",
        positions.len(),
        positions.len()
    );
}

/// Definitions that are not one SQL query, each with the text the parser stops on as the
/// definition writes it — for more than one statement, the first token of the statement
/// that may not be there. A row's `found` is `None` where the tokenizer, not the parser,
/// is what fails, or where the text simply runs out, so there is no token it stopped on.
///
/// The rows span the ways a definition can fail to be one query: text that is not SQL,
/// text the tokenizer cannot even lex, a statement of another kind, and more than one
/// statement.
const NOT_ONE_QUERY: &[(&str, &str, Option<&str>)] = &[
    ("prose", "the daily error count", Some("the")),
    ("a JSON document", "{\"query\": \"SELECT 1\"}", Some("{")),
    ("a bare value", "42", Some("42")),
    ("a truncated query", "SELECT * FROM", None),
    ("an unclosed string", "SELECT 'ER", None),
    ("whitespace only", "   ", None),
    ("a comment only", "-- nothing but a comment", None),
    ("a delete", "DELETE FROM logs WHERE level = 'ERROR'", Some("DELETE")),
    ("an insert", "INSERT INTO logs (id) VALUES (1)", Some("INSERT")),
    ("an update", "UPDATE logs SET level = 'INFO'", Some("UPDATE")),
    ("a DDL statement", "CREATE TABLE logs (id INT)", Some("CREATE")),
    ("two queries", "SELECT 1; SELECT 2", Some("SELECT")),
    (
        "a query followed by a mutation",
        "SELECT * FROM logs; DROP TABLE logs",
        Some("DROP"),
    ),
];

#[tokio::test]
async fn every_refusal_carries_the_parsers_own_error_and_not_one_constant_sentence() {
    let omni = Omni::new();
    let prefix = format!("{INVALID_VIEW_DEFINITION}{PARSER_ERROR}");
    let mut seen: Vec<(&str, Option<&str>, String)> = Vec::new();
    for (index, (case, definition, found)) in NOT_ONE_QUERY.iter().enumerate() {
        let name = view_name(&format!("diagnosed-{index}"));
        let message = refusal_message(&omni, case, &name, definition).await;
        assert!(
            message.starts_with(&prefix),
            "case {case:?}: expected the refusal to open with {prefix:?}, got {message:?}"
        );
        assert!(
            message.len() > prefix.len(),
            "case {case:?}: expected the parser's diagnosis after the prefix, got {message:?}"
        );
        if let Some(found) = found {
            assert!(
                message.contains(&format!("found: {found}")),
                "case {case:?}: expected the diagnosis to name the token {found:?} it stopped on, got {message:?}"
            );
        }
        assert_eq!(
            view_definition_rejection(definition).as_deref(),
            Some(message.as_str()),
            "case {case:?}: the definition rule and the handler must report one message"
        );
        seen.push((case, *found, message));
    }
    // Two defects the parser stops on different tokens for are diagnosed differently:
    // the message is read off the submitted text, not off a list of refusal classes.
    // (Two definitions that both carry no statement at all — whitespace, a comment — do
    // earn one diagnosis, because the parser stops at the same place in both.)
    for (case, found, message) in &seen {
        for (other_case, other_found, other_message) in &seen {
            if found == other_found {
                continue;
            }
            assert_ne!(
                message, other_message,
                "cases {case:?} and {other_case:?} stop on {found:?} and {other_found:?}, \
                 so they must not share the diagnosis {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_diagnosis_travels_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let definition = brace_at(1, 1);
    let expected = expected_brace_message(1, 1);
    let (status, body) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&view_name("diagnosed-over-the-wire"), &definition),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "case {definition:?}: the wire answered {status} rather than 400: {rendered:?}"
    );
    assert!(
        rendered.contains("ValidationException"),
        "case {definition:?}: the wire error body does not name ValidationException: {rendered:?}"
    );
    assert!(
        rendered.contains(&expected),
        "case {definition:?}: the wire error body must carry {expected:?}, got {rendered:?}"
    );
}

#[tokio::test]
async fn a_definition_that_is_one_query_earns_no_diagnosis() {
    // The rule refuses only what the parser cannot read as one query, so a query stays
    // admitted whatever form it takes — including the forms the rows above are built out
    // of, so the table is testing the defect and not the shape of the text.
    for (case, definition) in [
        ("a bare projection", "SELECT 1"),
        ("a trailing semicolon", "SELECT 1;"),
        ("a parenthesised query", "(SELECT 1)"),
        ("a query over several lines", "SELECT id\nFROM logs\nWHERE id > 1"),
        (
            "a leading comment",
            "-- the day's errors\nSELECT count(*) FROM logs",
        ),
        ("a set operation", "SELECT 1 UNION SELECT 2"),
    ] {
        let rejection = view_definition_rejection(definition);
        assert_eq!(
            rejection, None,
            "case {case:?}: one SQL query must be admitted, got {rejection:?}"
        );
    }
}
