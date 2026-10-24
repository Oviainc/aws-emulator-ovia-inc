// SPDX-License-Identifier: CC0-1.0

//! The `QueryStatement` a CloudTrail Lake dashboard widget carries: whether it is a
//! query CloudTrail Lake can run, and the one sentence a dashboard operation refuses
//! one that is not with.
//!
//! `CreateDashboard` and `UpdateDashboard` both take a `RequestWidgetList` and both
//! declare `InvalidQueryStatementException` — "The query that was submitted has
//! validation errors, or uses incorrect syntax or unsupported keywords"
//! (`smithy_query(query="shape",shape="InvalidQueryStatementException")`) — so the
//! reading of a statement, and the bound on how many widgets one request may carry,
//! live here once rather than in each handler, the way [`super::channels`] holds the
//! channel identifier reading. `StartQuery` reads the statement it RUNS through the
//! same reading ([`read_runnable_statement`]), which also answers with the tables the
//! parser read, because the event data store a run reads is whichever of them names
//! one.
//!
//! The statement is READ, not matched: the `sqlparser` crate parses it, so a statement
//! is judged by the SQL it IS rather than by any text this emulator searches for.
//! CloudTrail Lake runs SQL `SELECT` queries over event data stores, so a statement it
//! can run is ONE statement and that statement a query; anything the parser cannot
//! read, more than one statement, or a statement that is not a query — a write, a DDL
//! statement, a session command — is "incorrect syntax or unsupported keywords".
//!
//! Two things a CloudTrail Lake query may carry are not SQL, and both are substituted
//! before the parse rather than taught to the parser:
//!
//! * the documented query parameters ([`WIDGET_QUERY_PARAMETERS`]), each of which
//!   stands for a value CloudTrail binds when it runs the widget's query, so each
//!   reads as the parser's own bound-value placeholder;
//! * the event data store a widget selects FROM, which CloudTrail names by the id or
//!   the ARN of the store ("you can query across multiple event data stores as long as
//!   all event data stores exist in your account" — the model doc for
//!   `RequestWidget$QueryStatement`); neither form is a bare SQL identifier — an id can
//!   even begin with a digit, which no SQL dialect reads as the start of a name — so
//!   such a word is quoted as the identifier it is. Whether the store it names EXISTS
//!   is a different question, answered by `EventDataStoreNotFoundException`, not by
//!   this reading.
//!
//! The work one statement can cause is bounded before the parser is entered:
//! `QueryStatement` is `@length(1, 10000)` (`smithy_query(query="shape",
//! shape="QueryStatement")`), which the boundary validator enforces before any handler
//! runs, so the parse is over at most 10 000 characters; and
//! [`MAX_QUERY_NESTING_DEPTH`] and [`MAX_QUERY_TOKENS`] bound the DEPTH that statement
//! can ask for, in the parser and in the drop of the tree it builds, whatever
//! `sqlparser`'s own recursion limit admits. Both are counted on the tokens the
//! TOKENIZER read — one iterative pass that never recurses — so they hold before
//! anything descends.

use std::ops::ControlFlow;

use sqlparser::ast::{visit_relations, SetExpr, Statement};
use sqlparser::dialect::GenericDialect;
use sqlparser::keywords::Keyword;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::ops::types::RequestWidget;

/// Why a requested widget list is beyond what a custom dashboard carries, or `None`
/// when `count` is within [`super::CUSTOM_DASHBOARD_WIDGET_LIMIT`].
///
/// `Widgets` is the WHOLE list a dashboard will carry, not an addition to it, so the
/// bound applies to what the request sent — the same check for `CreateDashboard` and
/// `UpdateDashboard`, stated here beside the statement reading so the two paths cannot
/// drift. Each handler lifts the message into its own `ServiceQuotaExceededException`.
pub fn widgets_beyond_dashboard_limit_message(count: usize) -> Option<String> {
    (count > super::CUSTOM_DASHBOARD_WIDGET_LIMIT).then(|| {
        format!(
            "Widgets carries {count} widgets; a custom dashboard can have a maximum of {} \
             widgets",
            super::CUSTOM_DASHBOARD_WIDGET_LIMIT
        )
    })
}

/// The substitution tokens a widget's query statement may carry, from the model doc
/// for `RequestWidget$QueryParameters`: "The following query parameters are valid:
/// `$StartTime$`, `$EndTime$`, and `$Period$`."
pub const WIDGET_QUERY_PARAMETERS: [&str; 3] = ["$StartTime$", "$EndTime$", "$Period$"];

/// What a documented query parameter is read as: the SQL placeholder for a value
/// bound outside the statement, which is exactly what CloudTrail binds one as.
const SQL_PLACEHOLDER: &str = "?";

/// The refusal text a dashboard operation answers a statement it cannot run with.
///
/// Like [`super::DASHBOARD_NOT_FOUND_MESSAGE`] it is a FIXED service sentence — the
/// recorded `CreateDashboard` refusal of an unreadable widget query carries it and
/// reports no part of the statement — so both dashboard operations refuse alike rather
/// than wording their own.
pub const INVALID_QUERY_STATEMENT_MESSAGE: &str = "Query is not valid.";

/// The characters a WORD of a statement is made of, for the purpose of finding the
/// words that name an event data store: the characters an identifier, an id or an ARN
/// is written with.
///
/// `-`, `:` and `/` are in the set because an event data store id and an ARN carry
/// them; a word that carries them and names no event data store is passed through
/// unchanged, so including them changes nothing about how any other statement parses.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | ':' | '/' | '.')
}

/// Whether `word` names a resource rather than being a bare SQL identifier: an ARN,
/// or an event data store id.
///
/// The id shape is decided by [`super::is_event_data_store_id`], the crate's ONE
/// reading of an event data store id, so a statement names a store by exactly the
/// ids every other operation accepts.
fn names_a_resource(word: &str) -> bool {
    word.starts_with("arn:") || super::is_event_data_store_id(word)
}

/// `statement` as SQL: the documented query parameters read as placeholders, and every
/// word naming an event data store read as a quoted identifier.
///
/// The parameter substitution runs over the WHOLE statement first, so an occurrence
/// inside a string literal or a comment is rewritten too; that cannot change whether
/// the statement parses (a literal stays a literal and a comment stays a comment
/// either way), and the statement this emulator STORES is the caller's own text, never
/// this rewrite. Every other character is copied through untouched — a string literal,
/// an already-quoted identifier and a comment are each copied whole — so a statement
/// that names no resource is parsed exactly as it was written.
fn as_sql(statement: &str) -> String {
    let mut parameterised = statement.to_string();
    for parameter in WIDGET_QUERY_PARAMETERS {
        parameterised = parameterised.replace(parameter, SQL_PLACEHOLDER);
    }

    let mut sql = String::with_capacity(parameterised.len());
    let mut characters = parameterised.chars().peekable();
    while let Some(character) = characters.next() {
        // A COMMENT is copied through to its end before anything else is read, so a
        // quote or a resource-shaped word inside one is left exactly as written: an
        // apostrophe in `-- it's the console logins` opens no literal.
        if character == '-' && characters.peek() == Some(&'-') {
            sql.push(character);
            for inside in characters.by_ref() {
                sql.push(inside);
                if inside == '\n' {
                    break;
                }
            }
            continue;
        }
        if character == '/' && characters.peek() == Some(&'*') {
            sql.push(character);
            let mut previous = ' ';
            for inside in characters.by_ref() {
                sql.push(inside);
                if previous == '*' && inside == '/' {
                    break;
                }
                previous = inside;
            }
            continue;
        }
        match character {
            // A quoted span — a string literal, or an identifier the caller quoted
            // itself — is copied through to its closing quote, so nothing inside it is
            // read as a word. SQL escapes a quote inside such a span by DOUBLING it
            // (`'it''s'`), so a repeated quote continues the span rather than closing
            // it.
            '\'' | '"' | '`' => {
                sql.push(character);
                while let Some(inside) = characters.next() {
                    sql.push(inside);
                    if inside == character {
                        if characters.peek() == Some(&character) {
                            sql.push(character);
                            characters.next();
                            continue;
                        }
                        break;
                    }
                }
            }
            _ if is_word_character(character) => {
                let mut word = String::from(character);
                while characters.peek().is_some_and(|next| is_word_character(*next)) {
                    word.push(characters.next().unwrap_or_default());
                }
                if names_a_resource(&word) {
                    sql.push('"');
                    sql.push_str(&word);
                    sql.push('"');
                } else {
                    sql.push_str(&word);
                }
            }
            _ => sql.push(character),
        }
    }
    sql
}

/// The deepest BRACKETING one `QueryStatement` may carry, counted before the parser is
/// entered — and, on the same terms, the longest run of prefix operators it may apply
/// to one expression.
///
/// `pub` because this is the crate's ONE bound on the depth a `QueryStatement` may ask
/// the parser for, asserted by the test suites of both callers rather than repeated as
/// a number.
///
/// This is the EMULATOR's own bound, and it is what keeps a caller-controlled statement
/// from turning into a deep recursion — in the parser, or in the DROP of the tree it
/// builds, which descends with that tree whatever the parser's own recursion limit
/// refused. A statement of the documented form brackets a handful of levels (a filter
/// over an `IN (SELECT …)` with parenthesised predicates inside it is 3 or 4), so 32 is
/// an order of magnitude above any widget query and far below a descent that could
/// reach the end of a worker thread's stack. The sibling reading in
/// `services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state/query.rs`
/// bounds its own parse the same way.
pub const MAX_QUERY_NESTING_DEPTH: usize = 32;

/// The most TOKENS one `QueryStatement` may carry.
///
/// `pub` for the same reason as [`MAX_QUERY_NESTING_DEPTH`].
///
/// The general bound, for the nesting brackets do NOT express: a chain of binary or
/// unary operators, of set operations or of joins all nest the TREE at bracket depth
/// zero, and the DROP of that tree descends with it — so the token count, which bounds
/// the number of nodes the tree can have, is what bounds that descent. The widest
/// widget query the documented form writes is a few hundred tokens, so 4096 is an order
/// of magnitude above it, and a chain of that many nodes still unwinds inside a worker
/// thread's ordinary stack — `tests/all/dashboard_updates.rs` pins that with an
/// unbracketed operator chain of about 4000 tokens, just inside this bound, served
/// on the ordinary request path. It IS reachable inside the model's 10 000-character
/// bound (a projection list of two thousand short identifiers, say), and such a
/// statement is refused rather than read: a bound a caller can pass is the whole point
/// of having one, and nothing a dashboard widget queries needs that many tokens.
pub const MAX_QUERY_TOKENS: usize = 4_096;

/// The depth of a tokenized statement's bracketing: the most brackets open at any one
/// point, or the longest run of PREFIX operators applied to one expression, whichever
/// is greater — the two things that make a recursive-descent SQL parser, and the walk
/// of the tree it builds, descend.
///
/// Counted on the TOKENS the tokenizer read, so a bracket inside a string literal or a
/// comment is not one: the depth measured is the depth the parser would descend to.
/// Tokenizing is one iterative pass and never recurses, so this bound still costs less
/// than the parse it guards and is still applied before it.
///
/// A prefix operator descends only while it applies to the operator after it, so an
/// INFIX chain (`1 + 1 + 1`, whose operands sit between the operators) counts one, not
/// its length: its tree nests, but the token bound is what holds that.
fn nesting_depth(tokens: &[Token]) -> usize {
    let mut depth: usize = 0;
    let mut deepest: usize = 0;
    let mut prefix_run: usize = 0;
    let mut longest_prefix_run: usize = 0;
    for token in tokens {
        match token {
            Token::Whitespace(_) => {}
            Token::LParen | Token::LBracket => {
                depth += 1;
                deepest = deepest.max(depth);
                prefix_run = 0;
            }
            Token::RParen | Token::RBracket => {
                depth = depth.saturating_sub(1);
                prefix_run = 0;
            }
            Token::Minus | Token::Plus | Token::Tilde => {
                prefix_run += 1;
                longest_prefix_run = longest_prefix_run.max(prefix_run);
            }
            Token::Word(word) if word.keyword == Keyword::NOT => {
                prefix_run += 1;
                longest_prefix_run = longest_prefix_run.max(prefix_run);
            }
            _ => prefix_run = 0,
        }
    }
    deepest.max(longest_prefix_run)
}

/// The number of tokens a tokenized statement carries: the tokenizer's own tokens,
/// whitespace apart.
///
/// The parser builds at most one AST node per token, so this is what bounds the number
/// of nodes the tree can have — and a literal, however long, is ONE token, because the
/// tree it becomes is one node.
fn token_count(tokens: &[Token]) -> usize {
    tokens
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .count()
}

/// Whether `statement` is a query CloudTrail Lake can run: ONE SQL statement, and that
/// statement a query.
pub fn is_runnable_statement(statement: &str) -> bool {
    read_runnable_statement(statement).is_ok()
}

/// The depth the parser may descend to, stated here rather than left to the crate's
/// default so it cannot change under either caller.
///
/// It is above [`MAX_QUERY_NESTING_DEPTH`], so the EMULATOR's bound is the one a
/// statement meets first and a statement at that bound is one the parser still reads.
pub const QUERY_RECURSION_LIMIT: usize = 50;

/// The tables `statement` reads, or why it is no query CloudTrail Lake can run.
///
/// This is the crate's ONE reading of a `QueryStatement`, for both the dashboard
/// operations (which only need to know whether it is runnable) and `StartQuery` (which
/// needs the tables, because the event data store the run reads is whichever of them
/// names one). CloudTrail Lake runs SQL `SELECT` queries over event data stores, so a
/// runnable statement is ONE statement, that statement a query, and that query a READ:
/// the `WITH … INSERT` / `WITH … UPDATE` forms parse as a query whose body writes, and
/// a Lake query reads.
///
/// The tables answered with are the RELATIONS THE PARSER read, never a position in the
/// text, and only the single-part ones: a name in several parts (`catalog.table`) names
/// no event data store. A quoted identifier is answered unquoted, so an id or an ARN
/// reads back as the caller wrote it. Whether a table names a store that EXISTS is a
/// different question, answered by the registry.
///
/// The two structural bounds are checked on the TOKENS, before the parse, so a
/// statement that would turn into a deep recursion is refused rather than descended
/// with: tokenizing is iterative and never recurses, the parse is what descends. The
/// `Err` is the reason, for a trace record — never for the wire:
/// both callers answer [`INVALID_QUERY_STATEMENT_MESSAGE`].
pub fn read_runnable_statement(statement: &str) -> Result<Vec<String>, String> {
    let sql = as_sql(statement);
    let tokens = Tokenizer::new(&GenericDialect {}, &sql)
        .tokenize()
        .map_err(|error| error.to_string())?;

    let depth = nesting_depth(&tokens);
    if depth > MAX_QUERY_NESTING_DEPTH {
        return Err(format!(
            "the statement nests {depth} deep; a query may nest {MAX_QUERY_NESTING_DEPTH}"
        ));
    }
    let counted = token_count(&tokens);
    if counted > MAX_QUERY_TOKENS {
        return Err(format!(
            "the statement carries {counted} tokens; a query may carry {MAX_QUERY_TOKENS}"
        ));
    }

    let mut parser = Parser::new(&GenericDialect {})
        .with_recursion_limit(QUERY_RECURSION_LIMIT)
        .try_with_sql(&sql)
        .map_err(|error| error.to_string())?;
    let parsed = parser
        .parse_statements()
        .map_err(|error| error.to_string())?;
    let [Statement::Query(query)] = parsed.as_slice() else {
        return Err(format!(
            "a query is one SELECT statement; this one is {} statement(s) the parser read as \
             something else",
            parsed.len()
        ));
    };
    if !reads_only(&query.body) {
        return Err("a query reads an event data store, so it may not write".to_string());
    }

    let mut relations: Vec<String> = Vec::new();
    let _: ControlFlow<()> = visit_relations(query.as_ref(), |name| {
        let rendered = name.to_string();
        if !rendered.contains('.') {
            relations.push(rendered.trim_matches('"').to_string());
        }
        ControlFlow::Continue(())
    });
    Ok(relations)
}

/// Whether a parsed query body only READS.
///
/// `Statement::Query` is not enough on its own: the `WITH … INSERT` / `WITH … UPDATE`
/// forms parse as a query whose BODY writes.
fn reads_only(body: &SetExpr) -> bool {
    match body {
        SetExpr::Query(inner) => reads_only(&inner.body),
        SetExpr::SetOperation { left, right, .. } => reads_only(left) && reads_only(right),
        other => !matches!(other, SetExpr::Insert(_) | SetExpr::Update(_)),
    }
}

/// The first statement of `widgets` CloudTrail Lake cannot run, if any.
///
/// Both dashboard operations check the whole requested list before anything is stored,
/// so a request carrying one unreadable query persists none of its widgets.
pub fn first_unrunnable_statement(widgets: &[RequestWidget]) -> Option<&str> {
    widgets
        .iter()
        .map(|widget| widget.query_statement.as_str())
        .find(|statement| !is_runnable_statement(statement))
}
