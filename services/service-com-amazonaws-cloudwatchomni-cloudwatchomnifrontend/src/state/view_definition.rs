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
//! The refusal is worded the way this service words its other semantic refusals: one
//! sentence saying what the definition has to be, naming which of the three things is
//! wrong — the text is not SQL at all, it is not a query, or it is more than one
//! statement.

use ::sqlparser::ast::Statement;
use ::sqlparser::dialect::GenericDialect;
use ::sqlparser::parser::Parser;

/// The refusal for text a SQL parser cannot read as a query at all.
pub const NOT_A_QUERY: &str = "The view definition must be a valid SQL query.";

/// The refusal for ONE statement that is not a query.
pub const NOT_A_QUERY_STATEMENT: &str =
    "The view definition must be a SQL query, not a statement of another kind.";

/// Why `definition` is not a view definition, or `None` when it is one.
///
/// The parse is bounded twice over: `ViewDefinition`'s modelled `@length(1..10000)` caps
/// the text the tokenizer walks, and the parser's own recursion limit ends a deeply
/// nested expression with an error rather than a deep stack. Nothing here can panic on
/// caller text: the parser returns its failures.
pub fn view_definition_rejection(definition: &str) -> Option<String> {
    let statements = match Parser::parse_sql(&GenericDialect {}, definition) {
        Ok(statements) => statements,
        Err(_) => return Some(NOT_A_QUERY.to_string()),
    };
    match statements.as_slice() {
        // Text a SQL parser reads as no statement at all (whitespace, a bare comment)
        // defines nothing, so it is not a query either.
        [] => Some(NOT_A_QUERY.to_string()),
        [Statement::Query(_)] => None,
        // One statement that is not a query: a view IS the query it is defined by, so a
        // statement that does something else cannot define one.
        [_] => Some(NOT_A_QUERY_STATEMENT.to_string()),
        statements => Some(format!(
            "The view definition must be a single SQL query, not {} statements.",
            statements.len()
        )),
    }
}
