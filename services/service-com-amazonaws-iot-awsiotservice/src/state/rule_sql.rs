// SPDX-License-Identifier: CC0-1.0

//! A topic rule's SQL STATEMENT, and what it selects out of one published message.
//!
//! The statement is `SELECT <select clause> FROM '<topic filter>' [WHERE <condition>]` (the
//! AWS IoT SQL reference). The topic filter is a single-quoted literal, which no SQL grammar
//! parses as a relation, so the one span naming it ([`super::selected_topic`], this service's
//! lexer of that clause) is replaced by a relation name and the rest of the statement is READ
//! by the `sqlparser` crate: the select clause and the condition are its AST, never this
//! module's own parse of the text.
//!
//! What then runs over a message is the purpose-built walk in [`super::rule_expression`],
//! which reads the message document and nothing else. This module FAILS CLOSED around it: a
//! statement it cannot read, a clause an AWS IoT statement has no place for, an expression the
//! walk does not compute, and a projection whose payload is not a document are each
//! [`Selected::Undecided`], which the rules engine answers by performing NO action rather than
//! by approximating one. It is bounded too: the statement must fit
//! [`super::MAX_SQL_SCAN_CHARACTERS`], which caps this pass's work and its one allocation, and
//! nothing here panics on a caller's text — the reading answers `Option` and the walk answers
//! its own verdict.

use ::serde_json::{Map, Value};
use ::sqlparser::ast::{Expr, GroupByExpr, Select, SelectItem, SetExpr, Statement};
use ::sqlparser::dialect::GenericDialect;
use ::sqlparser::parser::Parser;

use super::rule_expression::{evaluate, Evaluated};

/// The relation name the topic filter's place is read as, so the rest of the statement parses
/// as SQL ([`parsed_statement`]).
const MESSAGE_RELATION: &str = "iot_message";

/// What a rule's statement selects out of ONE published message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selected {
    /// The message the statement selects, as the bytes every action is handed.
    Message(Vec<u8>),
    /// The statement's condition does not hold for this message.
    NotSelected,
    /// A statement this engine does not evaluate. No action is performed for it, because
    /// performing one would hand a target a message the rule never selected.
    Undecided,
}

/// What the rule whose statement is `sql` selects out of the message `payload`.
///
/// * `SELECT *` hands over the message AS PUBLISHED — the whole payload, byte for byte, the
///   one reading that is faithful for a payload that is not a JSON document either;
/// * a select clause naming attributes hands over a document carrying those attributes, each
///   under the name the clause gives it (its alias, else the last segment of the path), with
///   an attribute the message does not carry left out;
/// * a `WHERE` condition decides selection with SQL's three-valued logic: a condition that is
///   unknown for the message selects nothing, as it does in SQL.
pub fn selected_message(sql: &str, payload: &[u8]) -> Selected {
    let Some(parsed) = parsed_statement(sql) else {
        return Selected::Undecided;
    };
    // The message as a document, which a condition and a projection are evaluated over. A
    // payload that is not a document is only ever selected whole, by `SELECT *`.
    let document = ::serde_json::from_slice::<Value>(payload).ok();

    if let Some(condition) = &parsed.selection {
        let Some(document) = document.as_ref() else {
            return Selected::Undecided;
        };
        match evaluate(condition, document, 0) {
            Evaluated::Known(Value::Bool(true)) => {}
            Evaluated::Known(Value::Bool(false)) | Evaluated::Known(Value::Null) => {
                return Selected::NotSelected
            }
            // A condition unknown for this message selects nothing: a comparison against an
            // attribute the message does not carry is not a match.
            Evaluated::Absent => return Selected::NotSelected,
            Evaluated::Known(_) | Evaluated::Undecided => return Selected::Undecided,
        }
    }

    // `SELECT *` is the whole message, answered from the payload rather than from the
    // document read out of it.
    if parsed.projection.len() == 1 && matches!(parsed.projection[0], SelectItem::Wildcard(_)) {
        return Selected::Message(payload.to_vec());
    }
    let Some(document) = document.as_ref() else {
        return Selected::Undecided;
    };
    let mut selected = Map::new();
    for item in &parsed.projection {
        let (name, expression) = match item {
            SelectItem::UnnamedExpr(expression) => match selected_name(expression) {
                Some(name) => (name, expression),
                None => return Selected::Undecided,
            },
            SelectItem::ExprWithAlias { expr, alias } => (alias.value.clone(), expr),
            // A wildcard beside other items, and a qualified one, name a projection this
            // engine does not compute.
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(..) => {
                return Selected::Undecided
            }
        };
        match evaluate(expression, document, 0) {
            Evaluated::Known(value) => {
                selected.insert(name, value);
            }
            // An attribute the message does not carry is left out of the selected document.
            Evaluated::Absent => {}
            Evaluated::Undecided => return Selected::Undecided,
        }
    }
    Selected::Message(Value::Object(selected).to_string().into_bytes())
}

/// The clauses of a rule's statement this engine reads.
struct ParsedStatement {
    projection: Vec<SelectItem>,
    selection: Option<Expr>,
}

/// Read `sql` as a topic rule's statement, or `None` when it is not one this engine evaluates:
/// one longer than the prefix the topic-filter scan reads, one `sqlparser` does not parse, or
/// one carrying a clause an AWS IoT statement has no place for (`WITH`, a set operation,
/// `GROUP BY`, `HAVING`, `DISTINCT`, `ORDER BY`, a limit, a join).
fn parsed_statement(sql: &str) -> Option<ParsedStatement> {
    // Read at most one character past the bound, so an over-long statement is refused without
    // allocating the whole of it — the same `take` `super::selected_topic` scans under.
    let characters: Vec<char> = sql.chars().take(super::MAX_SQL_SCAN_CHARACTERS + 1).collect();
    if characters.len() > super::MAX_SQL_SCAN_CHARACTERS {
        return None;
    }
    let topic = super::selected_topic(sql)?;
    // The topic filter's span is the only part replaced, and both ends are read through `get`,
    // so no offset can index out of bounds.
    let standard: String = characters
        .get(..topic.start)?
        .iter()
        .copied()
        .chain(MESSAGE_RELATION.chars())
        .chain(characters.get(topic.end..)?.iter().copied())
        .collect();

    let statements = Parser::parse_sql(&GenericDialect {}, &standard).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    if query.with.is_some() || query.order_by.is_some() || query.limit_clause.is_some() {
        return None;
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    let select: &Select = select.as_ref();
    if select.having.is_some() || select.distinct.is_some() {
        return None;
    }
    if !matches!(
        &select.group_by,
        GroupByExpr::Expressions(expressions, modifiers)
            if expressions.is_empty() && modifiers.is_empty()
    ) {
        return None;
    }
    // One relation — the topic filter's place — and no join.
    let [from] = select.from.as_slice() else {
        return None;
    };
    if !from.joins.is_empty() || select.projection.is_empty() {
        return None;
    }
    Some(ParsedStatement {
        projection: select.projection.clone(),
        selection: select.selection.clone(),
    })
}

/// The name a selected expression carrying no alias is reported under: the last segment of the
/// attribute path it names, so `SELECT state.reported.temp` selects `temp`. `None` for an
/// expression that names no path — one this engine would have to name itself.
fn selected_name(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Identifier(identifier) => Some(identifier.value.clone()),
        Expr::CompoundIdentifier(identifiers) => identifiers.last().map(|last| last.value.clone()),
        Expr::Nested(inner) => selected_name(inner),
        _ => None,
    }
}
