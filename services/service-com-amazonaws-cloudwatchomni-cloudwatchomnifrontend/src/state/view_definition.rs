// SPDX-License-Identifier: CC0-1.0

//! What a view's `definition` has to be, and the refusal text a definition that is not
//! it renders.
//!
//! `ViewDefinition`'s model doc is "View definition (SQL query). Maximum 10,000
//! characters", and `CreateView`'s is "A view is a named, reusable SQL query that can be
//! referenced from telemetry queries". The model expresses only the definition's length,
//! so the rest of the rule is stated here: a definition is ONE SQL query. It is decided
//! by PARSING the definition with a real SQL parser rather than by inspecting its text,
//! so a query is admitted for being a query rather than for the shape of its source.
//!
//! The refusal is the PARSER's own reading of the text, under one fixed prefix: a client
//! that is told its definition was refused is told what the parser expected, which token
//! it stopped on and the line and column that token sits at, so the refusal points at
//! the defect in the submitted text instead of restating the rule. Nothing about the
//! diagnosis is written here — every variable part of it comes from the parse of the
//! request's own text.

use ::sqlparser::ast::Statement;
use ::sqlparser::dialect::{Dialect, GenericDialect};
use ::sqlparser::parser::{Parser, ParserError};
use ::sqlparser::tokenizer::Token;

/// The fixed text every definition refusal opens with; the parser's own rendering of
/// what is wrong with the definition follows it verbatim.
pub const INVALID_VIEW_DEFINITION: &str = "Invalid view definition: ";

/// Why `definition` is not a view definition, or `None` when it is one.
///
/// The parse is bounded twice over: `ViewDefinition`'s modelled `@length(1..10000)` caps
/// the text the tokenizer walks, and the parser's own recursion limit ends a deeply
/// nested expression with an error rather than a deep stack. Nothing here can panic on
/// caller text: the parser returns its failures, and they are what the caller is told.
pub fn view_definition_rejection(definition: &str) -> Option<String> {
    not_one_query(definition).map(|error| format!("{INVALID_VIEW_DEFINITION}{error}"))
}

/// The parser's error for text that is not one SQL query, or `None` when it is one.
fn not_one_query(definition: &str) -> Option<ParserError> {
    let dialect = GenericDialect {};
    let statements = match Parser::parse_sql(&dialect, definition) {
        Ok(statements) => statements,
        // Text no SQL parser can read as statements at all. ITS diagnosis is the
        // refusal: the expectation, the offending token and that token's position are
        // all facts about the submitted text, which is what the caller has to fix.
        Err(error) => return Some(error),
    };
    if matches!(statements.as_slice(), [Statement::Query(_)]) {
        return None;
    }
    Some(not_one_query_error(&dialect, definition, &statements))
}

/// The parser's error for text that IS SQL but is not one query.
///
/// What the statement-level parse read decides what the parser is asked for next, so the
/// refusal is in every case the parser's own reading of this text — its expectation, the
/// token it stopped on and that token's line and column — rather than a sentence
/// composed here.
fn not_one_query_error(
    dialect: &GenericDialect,
    definition: &str,
    statements: &[Statement],
) -> ParserError {
    let mut parser = match Parser::new(dialect).try_with_sql(definition) {
        Ok(parser) => parser,
        Err(error) => return error,
    };
    let stopped_on = match statements {
        // Nothing the parser reads as a statement — whitespace, a bare comment. Asked
        // for a statement, it reports what it expected and where the text ran out.
        [] => parser.parse_statement().err(),
        // One statement that is not a query. A view IS the query it is defined by, so
        // the parser is asked for the query body a definition has to be, and stops on
        // the keyword that starts the other kind of statement instead.
        [_] => parser.parse_query_body(dialect.prec_unknown()).err(),
        // More than one statement. Asked for the first statement and then for the end
        // of the text, the parser names the token the next statement starts at.
        _ => {
            let mut error = parser.parse_statement().err();
            if error.is_none() {
                // A statement delimiter is part of the statement before it, so what is
                // reported is the next statement's own first token.
                while parser.consume_token(&Token::SemiColon) {}
                error = parser.expect_token(&Token::EOF).err();
            }
            error
        }
    };
    stopped_on.unwrap_or_else(|| {
        // Unreachable: the statement-level parse read this text as something other than
        // one query, so the parse above cannot have read it as one.
        ParserError::ParserError(format!(
            "Expected: a single SQL query, found: {} statements",
            statements.len()
        ))
    })
}
