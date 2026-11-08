// SPDX-License-Identifier: CC0-1.0

//! Resolving a submission's named `Parameters` against the placeholders the SQL
//! it submits really references.
//!
//! The Data API binds parameters BY NAME: a statement references one as
//! `:<name>` and the request supplies it as a `SqlParameter` carrying that name.
//! So the request only makes sense when the two sets agree, and the service
//! resolves them against each other while it is still answering the submit — a
//! placeholder nothing supplies, a parameter nothing references and a name
//! supplied twice are all faults of the REQUEST, reported synchronously, so no
//! statement is ever recorded for a submission the service refused.
//!
//! Which names a statement references is read off the statement's own TOKENS,
//! from the SQL dialect's tokenizer ([`sqlparser`]) rather than from a scan of
//! the text: `:name` is a placeholder, `::type` is a cast, and a `:name` inside
//! a string literal or a comment is part of that literal or comment and
//! references nothing. A statement the tokenizer cannot read at all carries no
//! answer about its placeholders, so nothing is resolved for it and its text is
//! left to the engine, which is what reports a statement that does not parse.
//!
//! ## What bounds this pass
//!
//! Both of its inputs come straight from the request, so both are bounded
//! BEFORE anything is scanned, by the ENGINE's own ceilings on exactly those
//! two inputs (`within_engine_limits`): the largest statement text the engine
//! accepts and how many parameters one statement may bind. A submission past
//! either of them has nothing to resolve, because the engine itself is what
//! refuses it — so the work done here is bounded by those constants whatever
//! the request carries, and the number of distinct names collected is bounded
//! by the same parameter ceiling. Every lookup is a hash-set lookup rather than
//! a scan, so the work is linear in the bounded input and never the product of
//! its parts. The callers run it off the async worker, on the blocking pool, as
//! they run the engine itself.

use std::collections::HashSet;

use sqlparser::dialect::RedshiftSqlDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::ops::types::SqlParameter;

/// What a submission's `Parameters` and the SQL referencing them disagree about.
///
/// A submission is resolved in this order, which is the order the faults are
/// listed in: a name supplied twice makes the list itself unreadable, and a
/// placeholder with no value makes the statement unrunnable, while a value
/// nothing reads only makes the list wider than the SQL needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// One name appears more than once in `Parameters`.
    Duplicate(String),
    /// The SQL references these names and `Parameters` supplies none of them.
    Missing(Vec<String>),
    /// `Parameters` supplies these names and no statement references them.
    Unused(Vec<String>),
}

impl Mismatch {
    /// What the Data API refuses the submission with — the fixed sentence it
    /// states for this fault, carrying the names the request and the SQL
    /// disagree about.
    pub fn message(&self) -> String {
        match self {
            Mismatch::Duplicate(name) => format!("Found duplicate named parameter \"{name}\""),
            Mismatch::Missing(names) => format!(
                "Parameters list doesn't contain all parameters. Missing parameters: [{}]",
                names.join(", ")
            ),
            Mismatch::Unused(names) => format!(
                "Parameters list contains unused parameters. Unused parameters: [{}]",
                names.join(", ")
            ),
        }
    }
}

/// Resolve the named placeholders of `statements` against `parameters`.
///
/// `statements` is every statement of ONE submission: a single statement on its
/// own, or every statement of a batch, whose "parameters are available to all
/// SQL statements in the batch" and where "each statement can reference any
/// subset of the provided parameters"
/// (`smithy_query(query="doc",shape="BatchExecuteStatementInput",member="Parameters")`).
/// So a placeholder is supplied when ANY of them carries its name, and a
/// parameter is used when ANY statement references it.
///
/// BLOCKING, and bounded as the module documents: run it on the blocking pool.
pub fn resolve(statements: &[String], parameters: &[SqlParameter]) -> Result<(), Mismatch> {
    // Checked first, and on nothing but the two lengths, so a submission the
    // engine could never run is never scanned here.
    if !within_engine_limits(statements, parameters) {
        return Ok(());
    }

    // The names in the order the request supplies them, beside the set that
    // recognises one in constant time: a refusal lists them in the caller's own
    // order, and resolving them costs one lookup each.
    let mut supplied: Vec<&str> = Vec::with_capacity(parameters.len());
    let mut supplied_names: HashSet<&str> = HashSet::with_capacity(parameters.len());
    for parameter in parameters {
        if !supplied_names.insert(parameter.name.as_str()) {
            return Err(Mismatch::Duplicate(parameter.name.clone()));
        }
        supplied.push(parameter.name.as_str());
    }

    let mut referenced: Vec<String> = Vec::new();
    let mut referenced_names: HashSet<String> = HashSet::new();
    for sql in statements {
        // A statement whose tokens cannot be read, or that references more
        // distinct names than the engine can bind, says nothing this
        // resolution can use: it is left to the engine, which is what refuses
        // both.
        if !collect_named_placeholders(sql, &mut referenced, &mut referenced_names) {
            return Ok(());
        }
    }

    let missing: Vec<String> = referenced
        .iter()
        .filter(|name| !supplied_names.contains(name.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(Mismatch::Missing(missing));
    }

    let unused: Vec<String> = supplied
        .iter()
        .filter(|name| !referenced_names.contains(**name))
        .map(|name| (*name).to_string())
        .collect();
    if !unused.is_empty() {
        return Err(Mismatch::Unused(unused));
    }

    Ok(())
}

/// Whether this submission is one the engine could run at all: its parameter
/// list is no longer than the engine binds (`MAX_BOUND_PARAMETERS`) and no
/// statement of it is longer than the engine accepts (`MAX_SQL_BYTES`).
///
/// Both are read from the lengths alone, before any text is scanned, which is
/// what bounds everything this module then does. A submission past either
/// ceiling is not resolved here: the engine is where those limits live and what
/// reports a statement that breaks them.
fn within_engine_limits(statements: &[String], parameters: &[SqlParameter]) -> bool {
    parameters.len() <= bound_parameter_ceiling()
        && statements
            .iter()
            .all(|sql| usize::try_from(super::MAX_SQL_BYTES).is_ok_and(|max| sql.len() <= max))
}

/// How many parameters one statement may bind, as a count.
fn bound_parameter_ceiling() -> usize {
    usize::try_from(super::MAX_BOUND_PARAMETERS).unwrap_or(0)
}

/// Collect the names `sql` references into `referenced`, in the order it first
/// references each, recognising an already-collected one through
/// `referenced_names`.
///
/// Answers false for a statement the tokenizer could not read and for one
/// referencing more distinct names than the engine can bind — neither of which
/// this resolution can say anything about.
///
/// A placeholder is a colon immediately followed by a name, which the dialect's
/// tokenizer reports either as one placeholder token or as the two tokens it is
/// written with; anything else that begins with a colon — a cast's `::`, an
/// assignment's `:=`, a colon with a space after it — names no parameter.
fn collect_named_placeholders(
    sql: &str,
    referenced: &mut Vec<String>,
    referenced_names: &mut HashSet<String>,
) -> bool {
    let dialect = RedshiftSqlDialect {};
    let Ok(tokens) = Tokenizer::new(&dialect, sql).tokenize() else {
        return false;
    };
    let mut after_colon = false;
    for token in &tokens {
        let name = match token {
            Token::Colon => {
                after_colon = true;
                continue;
            }
            // A tokenizer that reads the whole `:name` as one placeholder
            // reports the name with its colon still on it.
            Token::Placeholder(placeholder) => placeholder.strip_prefix(':').unwrap_or_default(),
            Token::Word(word) if after_colon && word.quote_style.is_none() => word.value.as_str(),
            _ => {
                after_colon = false;
                continue;
            }
        };
        after_colon = false;
        if name.is_empty() || !referenced_names.insert(name.to_string()) {
            continue;
        }
        referenced.push(name.to_string());
        if referenced.len() > bound_parameter_ceiling() {
            return false;
        }
    }
    true
}
